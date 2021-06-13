#! /bin/bash
set -e

cargo build --release --target x86_64-unknown-linux-musl
cp target/x86_64-unknown-linux-musl/release/city17 bootstrap
chmod 755 bootstrap # actually applies 777 on WSL, I think, which is still valid
#zip -9 bootstrap.zip bootstrap
7za -sdel -bso0 -mx=9 a city17.zip bootstrap
# using 7zip gives better compression, so why not
