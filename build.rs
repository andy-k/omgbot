// Copyright (C) 2020-2023 Andy Kurnia.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::compile_protos(&["src/macondo.proto"], &["src/"])?;
    Ok(())
}
