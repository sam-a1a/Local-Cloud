import java.io.File

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// ---------------------------------------------------------------------------
// The engine.
//
// This app is the first consumer of the Rust engine that lives beside it, and
// consuming it means two artefacts that no `git clone` provides: a shared
// library per ABI, and the Kotlin that calls into it. Both are generated here,
// by an ordinary `./gradlew assembleDebug`, rather than by a script somebody
// has to remember to run. Neither is checked in - they are build output of the
// Rust, and a stale copy of either is a bug that presents as something else
// entirely.
// ---------------------------------------------------------------------------

/** `android/` is the Gradle root, so the Rust workspace is one directory up. */
val workspaceRoot: File = rootProject.projectDir.parentFile

/**
 * Only the ABIs that can actually run this.
 *
 * `minSdk` is 34, and every device that ships Android 14 is 64-bit, so
 * `armeabi-v7a` would be several minutes of cross-compilation per build for
 * hardware that cannot install the result. `x86_64` stays because that is the
 * emulator on an Intel host.
 */
val abis = listOf("arm64-v8a", "x86_64")

val ndkVersionUsed = "29.0.13846066"

val jniLibsDir = layout.buildDirectory.dir("rustJniLibs")
val bindingsDir = layout.buildDirectory.dir("generated/uniffi")

/**
 * Gradle started from Android Studio does not inherit a login shell's PATH, so
 * `cargo` is frequently not on it. Rustup puts it in a known place; fall back to
 * the bare name for anyone who installed it elsewhere and does have a PATH.
 */
val cargoBinDir = File(System.getProperty("user.home"), ".cargo/bin")
val cargo: String = File(cargoBinDir, "cargo").takeIf { it.exists() }?.absolutePath ?: "cargo"
val pathWithCargo: String =
    listOf(cargoBinDir.absolutePath, System.getenv("PATH").orEmpty()).joinToString(File.pathSeparator)

android {
    namespace = "com.ghazaleh.localcloud"
    compileSdk {
        version = release(37)
    }
    ndkVersion = ndkVersionUsed

    defaultConfig {
        applicationId = "com.ghazaleh.localcloud"
        minSdk = 34
        targetSdk = 37
        versionCode = 1
        versionName = "1.0"

        ndk {
            abiFilters += abis
        }
    }

    buildTypes {
        release {
            optimization {
                enable = false
            }
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
    buildFeatures {
        compose = true
    }

    sourceSets.getByName("main") {
        kotlin.directories.add(bindingsDir.get().asFile.absolutePath)
        jniLibs.directories.add(jniLibsDir.get().asFile.absolutePath)
    }
}

/**
 * Where the NDK is, according to the build rather than to this machine.
 *
 * Resolved from `ndkVersion` above, so the toolchain that cross-compiles the
 * engine is the same one AGP would use for any other native code, and a
 * missing NDK is reported as a missing NDK rather than as a compiler that
 * could not be found.
 *
 * Read while the build is being configured rather than while the task runs.
 * Deferring it would mean a lambda that captures this script, and a build
 * script is not something the configuration cache can serialise.
 */
val ndkDirectory = androidComponents.sdkComponents.ndkDirectory

/**
 * Cross-compiles the engine, one shared library per ABI, in the layout AGP
 * expects of a `jniLibs` directory.
 *
 * Always `--release`, including for a debug APK. A debug build of the engine is
 * not a slower version of the same thing: chunking and hashing a file is the
 * work, and unoptimised SHA-256 turns a transfer that should take seconds into
 * one that takes minutes. Debugging the engine is what the Rust tests are for.
 */
val cargoBuildEngine = tasks.register<Exec>("cargoBuildEngine") {
    group = "engine"
    description = "Cross-compiles the Rust engine to a shared library for each ABI."

    workingDir = workspaceRoot
    environment("PATH", pathWithCargo)
    environment("ANDROID_NDK_HOME", ndkDirectory.get().asFile.absolutePath)
    commandLine(
        buildList {
            add(cargo)
            add("ndk")
            abis.forEach { add("-t"); add(it) }
            add("--platform"); add("34")
            add("-o"); add(jniLibsDir.get().asFile.absolutePath)
            add("build"); add("--release")
            add("-p"); add("engine")
        }
    )

    inputs.dir(File(workspaceRoot, "engine/src")).withPathSensitivity(PathSensitivity.RELATIVE)
    inputs.file(File(workspaceRoot, "engine/Cargo.toml"))
    inputs.file(File(workspaceRoot, "engine/build.rs"))
    inputs.file(File(workspaceRoot, "Cargo.lock"))
    outputs.dir(jniLibsDir)
}

/**
 * Generates the Kotlin that calls into the library just built.
 *
 * uniffi reads the interface out of the compiled artefact rather than from a
 * description of it, so what this produces cannot claim a method the engine
 * does not export - and reading it from the ABI we are about to ship, rather
 * than from a host build, means it describes exactly the binary in the APK.
 *
 * The bindgen itself is a host binary, and a debug one deliberately: it runs
 * for a fraction of a second and compiling it optimised costs far more than it
 * ever saves.
 */
val generateUniffiBindings = tasks.register<Exec>("generateUniffiBindings") {
    group = "engine"
    description = "Generates the Kotlin bindings from the compiled engine library."
    dependsOn(cargoBuildEngine)

    workingDir = workspaceRoot
    environment("PATH", pathWithCargo)
    commandLine(
        cargo, "run", "--quiet", "-p", "engine", "--bin", "uniffi-bindgen", "--",
        "generate",
        "--library", File(jniLibsDir.get().asFile, "${abis.first()}/liblocalcloud.so").absolutePath,
        "--language", "kotlin",
        "--out-dir", bindingsDir.get().asFile.absolutePath,
    )

    inputs.dir(jniLibsDir)
    outputs.dir(bindingsDir)
}

tasks.named("preBuild") {
    dependsOn(generateUniffiBindings)
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.viewmodel.ktx)
    implementation(libs.androidx.lifecycle.process)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
    debugImplementation(libs.androidx.compose.ui.tooling)

    // The `@aar` variant, not the plain jar: it is the one that carries
    // libjnidispatch.so for each Android ABI, without which the generated
    // bindings cannot reach the engine at all.
    implementation(variantOf(libs.jna) { artifactType("aar") })

    testImplementation(libs.junit)
}
