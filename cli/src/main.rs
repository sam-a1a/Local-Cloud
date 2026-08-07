//! A terminal for driving one device.
//!
//! This exists to exercise the engine on real hardware. Pairing needs a code
//! shown on one machine and typed into another while both keep running, which
//! rules out a one-shot command and makes a prompt the natural shape.
//!
//! Ids are long - a device is 64 hex characters - so anything that takes one
//! accepts any unambiguous prefix, and items can also be named by their path,
//! which is what a person actually knows.

use anyhow::{anyhow, Result};
use localcloud::{Engine, EngineEvent, EventListener};
use std::io::{BufRead, Write};
use std::sync::Arc;

fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // A device's whole state lives under one directory, so pointing two
    // instances at different ones is all it takes to run both on one machine.
    let base = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let sync = format!("{}/sync_folder", base.trim_end_matches('/'));

    let engine = Arc::new(Engine::new(base, sync).map_err(to_anyhow)?);
    engine.set_event_listener(Arc::new(Printer));
    engine.start().map_err(to_anyhow)?;

    println!(
        "\n{} · {} · {}\nSync folder: {}\n",
        engine.device_name(),
        engine.device_platform(),
        engine.device_short_id(),
        engine.sync_dir()
    );
    println!("Type `help` for commands, `quit` to stop.\n");

    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        print!("> ");
        let _ = std::io::stdout().flush();

        line.clear();
        if stdin.lock().read_line(&mut line)? == 0 {
            break; // End of input: a piped script finished, or Ctrl-D.
        }

        let words: Vec<&str> = line.split_whitespace().collect();
        let Some((command, args)) = words.split_first() else {
            continue;
        };

        if matches!(*command, "quit" | "exit") {
            break;
        }

        if let Err(e) = run(&engine, command, args) {
            println!("{}", e);
        }
    }

    engine.stop();
    Ok(())
}

/// Prints everything the engine reports. On a device test the sequence of these
/// is most of the evidence, so nothing is filtered out.
struct Printer;

impl EventListener for Printer {
    fn on_event(&self, event: EngineEvent) {
        match event {
            // Progress is per block, and would otherwise bury everything else
            // for the length of a large transfer.
            EngineEvent::SendProgress { blocks_done, blocks_total, .. } => {
                if blocks_done == blocks_total || blocks_done % 32 == 0 {
                    println!("  · sent {}/{} blocks", blocks_done, blocks_total);
                }
            }
            EngineEvent::ReceiveProgress { blocks_done, blocks_total, .. } => {
                if blocks_done == blocks_total || blocks_done % 32 == 0 {
                    println!("  · received {}/{} blocks", blocks_done, blocks_total);
                }
            }
            other => println!("  [{:?}]", other),
        }
    }
}

fn to_anyhow(e: localcloud::EngineError) -> anyhow::Error {
    anyhow!("  {}", e)
}

fn run(engine: &Arc<Engine>, command: &str, args: &[&str]) -> Result<()> {
    match command {
        "help" => println!(
            "\n  devices                 devices visible on the network
  pair <device>           start pairing; shows a code to type on the other one
  offers                  devices asking to pair with this one
  accept <device> <code>  enter the code the other device is showing
  paired                  devices already paired
  unpair <device>

  ls                      the shared catalog, and who holds what
  import <path> [name]    copy a file in from outside the folder
  share <item> <device>…  send a copy to one or more devices
  pull <item>             take a copy for this device
  rm <item> [device]      delete a copy - this device's unless one is named

  trash                   items in the trash, with days remaining
  restore <item>
  purge <item>            destroy a trashed item now

  quit\n"
        ),

        "devices" => {
            let paired = engine.paired_devices();
            let devices = engine.visible_devices();
            if devices.is_empty() {
                println!("  nothing visible yet");
            }
            for d in devices {
                println!(
                    "  {}  {} ({}){}",
                    &d.device_id[..8],
                    d.name,
                    d.platform,
                    if paired.iter().any(|p| p.id == d.device_id) { "  · paired" } else { "" }
                );
            }
        }

        "paired" => {
            for d in engine.paired_devices() {
                println!("  {}  {} ({})", &d.id[..8], d.name, d.platform);
            }
        }

        "pair" => {
            let id = device_id(engine, arg(args, 0, "pair <device>")?)?;
            let code = engine.start_pairing(vec![id]).map_err(to_anyhow)?;
            println!(
                "\n  Code: {}\n  On the other device: accept {} {}\n",
                code,
                engine.device_short_id(),
                code
            );
        }

        "offers" => {
            let offers = engine.pairing_offers();
            if offers.is_empty() {
                println!("  nobody is asking");
            }
            for o in offers {
                println!("  {}  {} ({}) wants to pair", &o.device_id[..8], o.name, o.platform);
            }
        }

        "accept" => {
            let id = device_id(engine, arg(args, 0, "accept <device> <code>")?)?;
            let code = arg(args, 1, "accept <device> <code>")?;
            engine.confirm_pairing(id, code.to_string()).map_err(to_anyhow)?;
            println!("  code sent");
        }

        "unpair" => {
            let id = device_id(engine, arg(args, 0, "unpair <device>")?)?;
            engine.unpair(id).map_err(to_anyhow)?;
            println!("  unpaired");
        }

        "ls" => {
            let catalog = engine.catalog();
            if catalog.items.is_empty() {
                println!("  the catalog is empty");
            }
            for item in &catalog.items {
                let holders: Vec<String> = catalog
                    .holders
                    .iter()
                    .filter(|h| h.file_id == item.id)
                    .map(|h| holder_name(engine, h))
                    .collect();
                println!(
                    "  {}  {:<28} {:>9}  {}{}",
                    &item.id[..8],
                    item.path,
                    human(item.size),
                    if holders.is_empty() { "nobody holds it".to_string() } else { holders.join(", ") },
                    if item.is_trashed() { "  · trashed" } else { "" },
                );
            }
        }

        "import" => {
            let path = arg(args, 0, "import <path> [name]")?;
            let name = args.get(1).map(|s| s.to_string()).unwrap_or_else(|| {
                std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "imported".to_string())
            });
            let item = engine.import_file(path.to_string(), name).map_err(to_anyhow)?;
            println!("  imported as {}", item.path);
        }

        "share" => {
            let item = item_id(engine, arg(args, 0, "share <item> <device>…")?)?;
            if args.len() < 2 {
                return Err(anyhow!("  usage: share <item> <device>…"));
            }
            let targets: Result<Vec<String>> =
                args[1..].iter().map(|a| device_id(engine, a)).collect();
            engine.share_to(item, targets?).map_err(to_anyhow)?;
            println!("  sending");
        }

        "pull" => {
            let item = item_id(engine, arg(args, 0, "pull <item>")?)?;
            engine.pull_copy(item).map_err(to_anyhow)?;
            println!("  fetching");
        }

        "rm" => {
            let item = item_id(engine, arg(args, 0, "rm <item> [device]")?)?;
            match args.get(1) {
                Some(device) => {
                    let device = device_id(engine, device)?;
                    engine.delete_copy(item, device).map_err(to_anyhow)?;
                    println!("  requested");
                }
                None => {
                    let outcome = engine.delete_local_copy(item).map_err(to_anyhow)?;
                    if outcome.trashed {
                        println!("  that was the last copy, so it went to the trash");
                    } else {
                        println!("  deleted; {} copies remain", outcome.remaining_copies);
                    }
                }
            }
        }

        "trash" => {
            let trashed = engine.trashed_files();
            if trashed.is_empty() {
                println!("  the trash is empty");
            }
            for item in trashed {
                let left = engine
                    .trash_seconds_remaining(item.id.clone())
                    .map(|s| format!("{} days left", s / 86_400))
                    .unwrap_or_else(|| "live".to_string());
                println!("  {}  {:<28} {}", &item.id[..8], item.path, left);
            }
        }

        "restore" => {
            let item = item_id(engine, arg(args, 0, "restore <item>")?)?;
            engine.restore_file(item).map_err(to_anyhow)?;
            println!("  restored");
        }

        "purge" => {
            let item = item_id(engine, arg(args, 0, "purge <item>")?)?;
            engine.delete_permanently(item).map_err(to_anyhow)?;
            println!("  destroyed");
        }

        other => println!("  no such command: {} (try `help`)", other),
    }
    Ok(())
}

fn arg<'a>(args: &'a [&'a str], index: usize, usage: &str) -> Result<&'a str> {
    args.get(index)
        .copied()
        .ok_or_else(|| anyhow!("  usage: {}", usage))
}

/// Any unambiguous prefix of a device id, or a device's name.
fn device_id(engine: &Arc<Engine>, needle: &str) -> Result<String> {
    let mut candidates: Vec<(String, String)> = engine
        .visible_devices()
        .into_iter()
        .map(|d| (d.device_id, d.name))
        .collect();

    // Paired devices that have gone quiet, and devices asking to pair, are both
    // things a person still needs to be able to name.
    for d in engine.paired_devices() {
        if !candidates.iter().any(|(id, _)| *id == d.id) {
            candidates.push((d.id, d.name));
        }
    }
    for o in engine.pairing_offers() {
        if !candidates.iter().any(|(id, _)| *id == o.device_id) {
            candidates.push((o.device_id, o.name));
        }
    }

    resolve(needle, candidates, "device")
}

/// Any unambiguous prefix of an item id, or its path.
fn item_id(engine: &Arc<Engine>, needle: &str) -> Result<String> {
    let mut items: Vec<(String, String)> = engine
        .catalog()
        .items
        .into_iter()
        .map(|f| (f.id, f.path))
        .collect();
    for f in engine.trashed_files() {
        if !items.iter().any(|(id, _)| *id == f.id) {
            items.push((f.id, f.path));
        }
    }

    resolve(needle, items, "item")
}

/// One id, or an explanation of why the answer was not exactly one.
///
/// Refusing an ambiguous prefix matters more than the convenience does: quietly
/// taking the first match would send a file to the wrong device, and during a
/// device test that is indistinguishable from a bug in the engine.
fn resolve(needle: &str, candidates: Vec<(String, String)>, what: &str) -> Result<String> {
    let matches: Vec<&(String, String)> = candidates
        .iter()
        .filter(|(id, label)| id.starts_with(needle) || label == needle)
        .collect();

    match matches.as_slice() {
        [(id, _)] => Ok(id.clone()),
        [] => Err(anyhow!("  nothing matches \"{}\" ({})", needle, what)),
        several => {
            let names: Vec<String> = several
                .iter()
                .map(|(id, label)| format!("{} ({})", &id[..8], label))
                .collect();
            Err(anyhow!("  \"{}\" matches several: {}", needle, names.join(", ")))
        }
    }
}

fn holder_name(engine: &Arc<Engine>, holder: &localcloud::FileHolder) -> String {
    if holder.device_id == engine.device_id() {
        return "this device".to_string();
    }
    engine
        .paired_devices()
        .into_iter()
        .find(|d| d.id == holder.device_id)
        .map(|d| d.name)
        .unwrap_or_else(|| holder.device_id[..8].to_string())
}

fn human(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}
