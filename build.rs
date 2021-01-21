// Copyright (C) 2020-2021 Andy Kurnia. All rights reserved.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::compile_protos(&["src/macondo.proto"], &["src/"])?;
    Ok(())
}
