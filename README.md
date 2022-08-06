# OMGBot

## About

OMGBot, Andy Kurnia's bot that can play OMGWords game on the Woogles.io
platform using Kurnia algorithm and data structures.

## License

Copyright (C) 2020-2022 Andy Kurnia.\
Released under the MIT license.

Bugs included.

## Initial Setup

Put `macondo.proto` in `src/`. This file is in a different project.
https://github.com/domino14/macondo/blob/001a86986f7e73ea4b4544443d7a8a30ffa4f1ea/api/proto/macondo/macondo.proto

Generate `*.kwg` and `*.klv` files, put them in current directory when running.
(Refer to the wolges project.)

```
cargo run --release
```

## GitHub Badge

- [![Rust](https://github.com/andy-k/omgbot/actions/workflows/rust.yml/badge.svg)](https://github.com/andy-k/omgbot/actions/workflows/rust.yml)
