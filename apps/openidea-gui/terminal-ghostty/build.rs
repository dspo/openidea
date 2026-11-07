//! Build libghostty-vt and link it statically (M11's integrated terminal core).
//!
//! Resolution order, per input:
//!
//! - **Ghostty sources**: `GHOSTTY_SRC` (a local checkout — the dev loop, where
//!   zig's own cache makes each rebuild incremental and *every* build-script
//!   run re-checks the checkout) or a pinned upstream tarball downloaded into
//!   the build cache (the CI path; sha256-verified).
//! - **Zig**: `GHOSTTY_ZIG` → a `zig` on PATH with version ≥ 0.17 (Ghostty's
//!   own floor) → a pinned Zig toolchain downloaded into the build cache.
//!
//! The build cache is `OUT_DIR` by default and `TERMINAL_GHOSTTY_CACHE` when set.
//! CI points that at a directory `actions/cache` restores, so the zig build is
//! paid once per (Ghostty rev, zig identity) instead of once per job. A stamp
//! beside the artifact records what produced it: the fast path is taken only
//! when both still match, so changing either one rebuilds instead of linking a
//! library the bindings in `src/sys.rs` were not transcribed against.
//!
//! Force a rebuild with `cargo clean -p terminal-ghostty`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Ghostty rev the FFI bindings in `src/sys.rs` were transcribed against.
/// Bump this together with the bindings (diff `include/ghostty/vt/*.h`).
const GHOSTTY_REV: &str = "042b16b6146ada29877dfeeea9f286889e4adeb1";
const GHOSTTY_TARBALL_SHA256: &str = "59055c41ac0be16a1e5bb2079312bac89181b94f82fd0399ca81682ea047b0c0";
/// Ghostty requires zig ≥ 0.17 (`build.zig.zon` `minimum_zig_version`).
const ZIG_VERSION: &str = "0.17.0";
const ZIG_MIN: (u32, u32) = (0, 17);

const LIB_NAME: &str = "libghostty-vt.a";
const STAMP_NAME: &str = "ghostty-vt.stamp";

fn main() {
    println!("cargo:rerun-if-env-changed=GHOSTTY_SRC");
    println!("cargo:rerun-if-env-changed=GHOSTTY_ZIG");
    println!("cargo:rerun-if-env-changed=TERMINAL_GHOSTTY_CACHE");
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let cache = cache_dir(&out);
    // A cache restored by `actions/cache` exists, but a miss (and a fresh local
    // build with `TERMINAL_GHOSTTY_CACHE` set) does not: every download below
    // writes here, so the directory has to exist first.
    std::fs::create_dir_all(&cache).expect("create the ghostty-vt build cache directory");
    let lib_dir = cache.join("lib");

    // The dev loop edits a checkout in place, which no stamp can observe: every
    // build-script run reaches zig (whose own cache does the real work).
    let dev = std::env::var_os("GHOSTTY_SRC");
    if let Some(src) = &dev {
        println!("cargo:rerun-if-changed={}", Path::new(src).display());
    }

    let want = format!("{GHOSTTY_REV}\n{}\n", zig_identity());
    if dev.is_none() && lib_dir.join(LIB_NAME).exists() && read_stamp(&cache) == want {
        link(&export_static(&cache));
        return;
    }

    let src = resolve_sources(&cache);
    let zig = resolve_zig(&cache);
    build_libvt(&zig, &src, &cache);
    let _ = std::fs::write(cache.join(STAMP_NAME), &want);
    link(&export_static(&cache));
}

/// The artifact cache: `TERMINAL_GHOSTTY_CACHE` when set (CI restores it), with a
/// relative value resolved against `OUT_DIR` so a workspace-relative path
/// works; otherwise `OUT_DIR` itself.
fn cache_dir(out: &Path) -> PathBuf {
    match std::env::var_os("TERMINAL_GHOSTTY_CACHE") {
        Some(p) => {
            let p = PathBuf::from(p);
            if p.is_absolute() { p } else { out.join(p) }
        }
        None => out.to_path_buf(),
    }
}

fn read_stamp(cache: &Path) -> String {
    std::fs::read_to_string(cache.join(STAMP_NAME)).unwrap_or_default()
}

fn link(lib: &Path) {
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=objc");
    }
}

/// Export the archive into a directory of its own and hand that to the linker.
///
/// `zig build -Demit-lib-vt` writes `libghostty-vt.dylib` beside the archive, and
/// ld64 resolves a bare `-lghostty-vt` to the dylib when both sit in one search
/// path. The result is a binary with an `LC_LOAD_DYLIB` on a shared library
/// nothing bundles: shipped apps fail to launch and test binaries fail to start.
/// This directory contains only the archive, so `-l` can only mean static.
fn export_static(cache: &Path) -> PathBuf {
    let src = cache.join("lib").join(LIB_NAME);
    let dir = cache.join("lib-static");
    let dst = dir.join(LIB_NAME);
    std::fs::create_dir_all(&dir).expect("create lib-static");
    if std::fs::metadata(&dst).map(|m| m.len()).ok() != std::fs::metadata(&src).map(|m| m.len()).ok() {
        std::fs::copy(&src, &dst).expect("export libghostty-vt.a");
    }
    dir
}

fn build_libvt(zig: &Path, src: &Path, cache: &Path) {
    let status = Command::new(zig)
        .args([
            "build",
            "-Demit-lib-vt",
            "-Doptimize=ReleaseFast",
            // Zig targets the host CPU by default, which bakes the build
            // machine's features into the archive; it is cached across CI
            // runners and shipped in the Linux packages, where a CPU without
            // those features dies with SIGILL. Ghostty's own Linux packaging
            // passes this flag too (flatpak manifests), and its test builds
            // resolve `Config.baselineTarget` for the same reason.
            "-Dcpu=baseline",
            "--prefix",
        ])
        .arg(cache)
        .current_dir(src)
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn zig ({}): {e}", zig.display()));
    assert!(status.success(), "zig build -Demit-lib-vt failed");
    assert!(cache.join("lib").join(LIB_NAME).exists(), "zig build produced no {LIB_NAME}");
}

// ---------------------------------------------------------------------------
// Ghostty sources
// ---------------------------------------------------------------------------

fn resolve_sources(cache: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("GHOSTTY_SRC") {
        let src = PathBuf::from(p);
        assert!(
            src.join("build.zig").exists(),
            "GHOSTTY_SRC ({}) is not a Ghostty checkout (no build.zig)",
            src.display()
        );
        return src;
    }
    download_sources(cache)
}

/// Download the pinned Ghostty tarball (codeload archive of `GHOSTTY_REV`)
/// and extract it under `<cache>/ghostty-src`.
fn download_sources(cache: &Path) -> PathBuf {
    let dir = cache.join("ghostty-src");
    if let Some(p) = find_checkout(&dir) {
        return p;
    }

    let url = format!("https://codeload.github.com/ghostty-org/ghostty/tar.gz/{GHOSTTY_REV}");
    let tarball = cache.join("ghostty-src.tar.gz");
    fetch(&url, &tarball);
    assert_sha256(&tarball, GHOSTTY_TARBALL_SHA256);
    std::fs::create_dir_all(&dir).unwrap();
    run(Command::new("tar").arg("-xzf").arg(&tarball).arg("-C").arg(&dir));
    // The extracted tree is the artifact; the archive only re-verifies the
    // download, and keeping it would put another 90 MB in the CI cache.
    let _ = std::fs::remove_file(&tarball);
    find_checkout(&dir).expect("ghostty tarball extracted but no build.zig found")
}

fn find_checkout(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.join("build.zig").exists())
}

// ---------------------------------------------------------------------------
// Zig toolchain
// ---------------------------------------------------------------------------

/// The zig identity the stamp records, resolved without side effects so a
/// cached artifact can be validated before anything is downloaded.
fn zig_identity() -> String {
    if let Ok(p) = std::env::var("GHOSTTY_ZIG") {
        return format!("env:{p}");
    }
    if let Some((path, v)) = path_zig() {
        return format!("path:{}:{}.{}", path.display(), v.0, v.1);
    }
    format!("pinned:{ZIG_VERSION}")
}

/// A `zig` on PATH new enough for Ghostty, if any.
fn path_zig() -> Option<(PathBuf, (u32, u32))> {
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = dir.join("zig");
        if candidate.is_file()
            && let Some(v) = zig_version(&candidate)
            && v >= ZIG_MIN
        {
            return Some((candidate, v));
        }
    }
    None
}

fn resolve_zig(cache: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("GHOSTTY_ZIG") {
        return PathBuf::from(p);
    }
    if let Some((path, _)) = path_zig() {
        return path;
    }
    download_zig(cache)
}

fn zig_version(zig: &Path) -> Option<(u32, u32)> {
    let out = Command::new(zig).arg("version").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut parts = s.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Host zig tarball identifiers for the platforms this workspace CI builds on.
fn zig_tarball_id() -> (&'static str, &'static str) {
    match (std::env::var("CARGO_CFG_TARGET_OS").as_deref(), std::env::var("CARGO_CFG_TARGET_ARCH").as_deref()) {
        (Ok("macos"), Ok("aarch64")) => {
            ("aarch64-macos", "b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a")
        }
        (Ok("linux"), Ok("x86_64")) => {
            ("x86_64-linux", "1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026")
        }
        (os, arch) => {
            panic!("no pinned zig toolchain for os={os:?} arch={arch:?}; install zig >= 0.17 and set GHOSTTY_ZIG")
        }
    }
}

fn download_zig(cache: &Path) -> PathBuf {
    let (id, sha256) = zig_tarball_id();
    let dir = cache.join("zig");
    let marker = dir.join(format!(".ready-{ZIG_VERSION}-{id}"));
    if marker.exists() {
        return zig_binary(&dir, id);
    }

    let url = format!("https://ziglang.org/download/{ZIG_VERSION}/zig-{id}-{ZIG_VERSION}.tar.xz");
    let tarball = cache.join(format!("zig-{id}.tar.xz"));
    fetch(&url, &tarball);
    assert_sha256(&tarball, sha256);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    run(Command::new("tar").arg("-xJf").arg(&tarball).arg("-C").arg(&dir));
    let _ = std::fs::remove_file(&tarball);
    std::fs::write(&marker, b"").unwrap();
    zig_binary(&dir, id)
}

fn zig_binary(dir: &Path, id: &str) -> PathBuf {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path().join("zig"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("zig toolchain {id} extracted but no zig binary found"))
}

// ---------------------------------------------------------------------------
// fetch + verify helpers (curl/tar/sha256sum are present on both CI legs)
// ---------------------------------------------------------------------------

fn fetch(url: &str, dest: &Path) {
    run(Command::new("curl").args(["-fSL", "--retry", "3", "-o"]).arg(dest).arg(url));
}

fn run(cmd: &mut Command) {
    let status = cmd.status().unwrap_or_else(|e| panic!("failed to spawn {:?}: {e}", cmd));
    assert!(status.success(), "{cmd:?} failed");
}

fn assert_sha256(path: &Path, expected: &str) {
    let got = if which("sha256sum") {
        let out = Command::new("sha256sum").arg(path).output().expect("sha256sum");
        String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string()
    } else {
        let out = Command::new("shasum").args(["-a", "256"]).arg(path).output().expect("shasum");
        String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string()
    };
    assert_eq!(got, expected, "sha256 mismatch for {}", path.display());
}

fn which(prog: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|dir| dir.join(prog).is_file())
}
