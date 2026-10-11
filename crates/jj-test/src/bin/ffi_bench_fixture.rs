//! Builds the FFI benchmark repository: `ffi-bench-fixture <path>` replaces anything at `<path>`.

use std::path::PathBuf;

fn main() {
    let path: PathBuf = std::env::args_os()
        .nth(1)
        .expect("usage: ffi-bench-fixture <path>")
        .into();
    if path.exists() {
        std::fs::remove_dir_all(&path).expect("remove previous fixture");
    }
    jj_test::ffi_bench::build_ffi_bench_repo(&path);
    println!("{}", path.display());
}
