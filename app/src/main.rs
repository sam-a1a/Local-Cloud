use dioxus::prelude::*;
use localcloud::db::{FileHolder, PairedDevice};
use localcloud::{Engine, FileMetadata};
use std::sync::Arc;

fn main() {
    // Route panics/logs to logcat (and to stdout on desktop) so we never
    // see an opaque "Any { .. }" again.
    dioxus::logger::initialize_default();
    launch(app);
}

#[cfg(target_os = "android")]
fn android_files_dir() -> Result<std::path::PathBuf, String> {
    use jni::objects::{JObject, JString};

    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|e| format!("JavaVM::from_raw failed: {e:?}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach_current_thread failed: {e:?}"))?;
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let files_dir = env
        .call_method(&activity, "getFilesDir", "()Ljava/io/File;", &[])
        .map_err(|e| format!("getFilesDir call failed: {e:?}"))?
        .l()
        .map_err(|e| format!("getFilesDir result not an object: {e:?}"))?;

    let path_jstring: JString = env
        .call_method(&files_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| format!("getAbsolutePath call failed: {e:?}"))?
        .l()
        .map_err(|e| format!("getAbsolutePath result not an object: {e:?}"))?
        .into();

    let path_string: String = env
        .get_string(&path_jstring)
        .map_err(|e| format!("get_string failed: {e:?}"))?
        .into();

    Ok(std::path::PathBuf::from(path_string))
}

fn get_dirs() -> Result<(String, String), String> {
    #[cfg(target_os = "android")]
    {
        let root = android_files_dir()?;
        let base = root.join("localcloud_data");
        let sync = root.join("localcloud_sync");
        std::fs::create_dir_all(&base)
            .map_err(|e| format!("create_dir_all({base:?}) failed: {e:?}"))?;
        std::fs::create_dir_all(&sync)
            .map_err(|e| format!("create_dir_all({sync:?}) failed: {e:?}"))?;
        Ok((base.to_string_lossy().to_string(), sync.to_string_lossy().to_string()))
    }
    #[cfg(not(target_os = "android"))]
    {
        let root = dirs::data_dir().ok_or_else(|| "dirs::data_dir() returned None".to_string())?;
        let base = root.join("LocalCloudData");
        let sync = root.join("LocalCloudSync");
        std::fs::create_dir_all(&base)
            .map_err(|e| format!("create_dir_all({base:?}) failed: {e:?}"))?;
        std::fs::create_dir_all(&sync)
            .map_err(|e| format!("create_dir_all({sync:?}) failed: {e:?}"))?;
        Ok((base.to_string_lossy().to_string(), sync.to_string_lossy().to_string()))
    }
}

fn app() -> Element {
    // Initialize engine once and provide it to the context tree.
    let engine = use_context_provider(|| {
        let (base, sync) = get_dirs().unwrap_or_else(|e| {
            log::error!("get_dirs() failed: {e}");
            panic!("get_dirs() failed: {e}");
        });

        log::info!("Using base={base} sync={sync}");

        let engine = Engine::new(base.clone(), sync.clone()).unwrap_or_else(|e| {
            log::error!("Engine::new(base={base}, sync={sync}) failed: {e:?}");
            panic!("Engine::new failed: {e:?}");
        });

        let engine = Arc::new(engine);

        if let Err(e) = engine.start() {
            log::error!("engine.start() failed: {e:?}");
            panic!("engine.start() failed: {e:?}");
        }

        engine
    });

    // UI State
    let mut files = use_signal(Vec::new);
    let mut holders = use_signal(Vec::new);
    let mut peers = use_signal(Vec::new);
    let mut paired = use_signal(Vec::new);
    let mut logs = use_signal(Vec::new);

    // Dedicated clone for the background poller so `engine` stays usable
    // everywhere else in this component.
    let poll_engine = engine.clone();
    use_future(move || {
        let poll_engine = poll_engine.clone();
        async move {
            loop {
                let eng = poll_engine.clone();
                let event = tokio::task::spawn_blocking(move || eng.poll_event(500)).await.unwrap();

                if let Some(event) = event {
                    let mut l = logs.write();
                    l.push(format!("{:?}", event));
                    if l.len() > 20 {
                        l.remove(0);
                    }
                }

                let catalog = poll_engine.get_catalog();
                files.write().clone_from(&catalog.files);
                holders.write().clone_from(&catalog.holders);
                peers.write().clone_from(&poll_engine.get_known_peers());
                paired.write().clone_from(&poll_engine.paired_devices());
            }
        }
    });

    rsx! {
        div { padding: "20px", font_family: "sans-serif",
            h1 { "LocalCloud Mesh" }
            p { font_size: "12px", color: "gray",
                "{engine.device_name()} · {engine.device_platform()} · {engine.device_short_id()}"
            }

            h2 { "Devices on this network ({peers.read().len()})" }
            ul {
                for peer in peers.read().iter() {
                    li {
                        "{peer.name} · {peer.platform}"
                        if paired.read().iter().any(|d: &PairedDevice| d.id == peer.device_id) {
                            span { color: "green", " · paired" }
                        } else {
                            span { color: "gray", " · not paired" }
                        }
                    }
                }
            }

            h2 { "Catalog ({files.read().len()})" }
            table { border_collapse: "collapse", width: "100%",
                thead {
                    tr {
                        th { text_align: "left", border_bottom: "1px solid black", "Name" }
                        th { text_align: "left", border_bottom: "1px solid black", "Size" }
                        th { text_align: "left", border_bottom: "1px solid black", "Copies on" }
                        th { text_align: "left", border_bottom: "1px solid black", "Actions" }
                    }
                }
                tbody {
                    for row in catalog_rows(&files.read(), &holders.read(), &paired.read(), &engine) {
                        { file_row(row.0, row.1, engine.clone()) }
                    }
                }
            }

            div { margin_top: "20px",
                button {
                    onclick: {
                        let engine = engine.clone();
                        move |_| {
                            let f = files.read();
                            let d = paired.read();
                            match (f.first(), d.first()) {
                                (Some(file), Some(device)) => {
                                    let device: &PairedDevice = device;
                                    println!("[UI] Sharing {} with {}", file.path, device.name);
                                    if let Err(e) =
                                        engine.share_to(file.id.clone(), vec![device.id.clone()])
                                    {
                                        println!("[UI] Share failed: {}", e);
                                    }
                                }
                                (None, _) => println!("[UI] Nothing in the catalog yet"),
                                (_, None) => println!("[UI] No paired devices yet"),
                            }
                        }
                    },
                    "Share first item with first paired device"
                }
            }

            h3 { "Event Logs" }
            pre { background_color: "#f4f4f4", padding: "10px", overflow_x: "auto",
                for log in logs.read().iter() {
                    "{log}\n"
                }
            }
        }
    }
}

/// Pairs each catalog item with a readable summary of who holds it.
///
/// A copy whose content hash differs from the item's is marked, because copies
/// are snapshots that never update themselves - a device can hold a version
/// several edits behind, and the catalog should say so rather than implying
/// every holder is current.
fn catalog_rows(
    files: &[FileMetadata],
    holders: &[FileHolder],
    devices: &[PairedDevice],
    engine: &Engine,
) -> Vec<(FileMetadata, String)> {
    let my_id = engine.device_id();
    let my_name = engine.device_name();

    files
        .iter()
        .map(|file| {
            let mut names: Vec<String> = holders
                .iter()
                .filter(|h| h.file_id == file.id)
                .map(|h| {
                    let name = if h.device_id == my_id {
                        my_name.clone()
                    } else {
                        devices
                            .iter()
                            .find(|d| d.id == h.device_id)
                            .map(|d| d.name.clone())
                            .unwrap_or_else(|| h.device_id.chars().take(8).collect())
                    };

                    if h.content_hash == file.content_hash {
                        name
                    } else {
                        format!("{} (older)", name)
                    }
                })
                .collect();
            names.sort();

            let summary = if names.is_empty() {
                "no copies".to_string()
            } else {
                names.join(", ")
            };

            (file.clone(), summary)
        })
        .collect()
}

fn file_row(file: FileMetadata, holders: String, engine: Arc<Engine>) -> Element {
    let path = file.path.clone();
    let size = file.size;
    let file_id = file.id.clone();

    rsx! {
        tr {
            td { "{path}" }
            td { "{size}" }
            td { "{holders}" }
            td {
                button {
                    onclick: move |_| {
                        let fid = file_id.clone();
                        let eng = engine.clone();
                        tokio::spawn(async move {
                            let _ = eng.fetch_file_on_demand(fid);
                        });
                    },
                    "Fetch"
                }
            }
        }
    }
}