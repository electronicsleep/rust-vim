# rust-vim

Simple Rust and Vim GUI editor for learning

Hyper efficient with essential Vim support

Verified working quicky on even a Raspberry Pi 400

Using the most productive vim motions

Vim toggle and on screen help for learning

Minimal dependencies

Works on Mac and Linux

Run and open file or folders

Run via cargo
```
cargo run -- README.md
cargo run -- .
```

Build and open file or folder

```
cargo build --release
sudo cp ./target/release/rust-vim /usr/local/bin/

rust-vim README.md
rust-vim .
```

![rust-vim screenshot](screenshot.jpg)

https://www.egui.rs/
