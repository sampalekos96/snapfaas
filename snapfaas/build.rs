use std::io::Result;
fn main() -> Result<()> {
    println!("cargo:rustc-link-lib=dylib=fdt");
    prost_build::compile_protos(&["src/syscalls.proto"], &["src/"])?;
    Ok(())
}
