#!/bin/sh 

set -e
set -x

cargo build --release --target arm-unknown-linux-musleabihf
arm-linux-gnueabi-strip -S target/arm-unknown-linux-musleabihf/release/fridgyeast
cat target/arm-unknown-linux-musleabihf/release/fridgyeast | ssh ferment.local "mv -f fridgyeast fridgyeast.old && cat > fridgyeast && chmod u+x fridgyeast && (tmux kill-session -t ferment; tmux new -d -s ferment ./fridgyeast; sleep 0.3; tail -20 fridgyeast.log)"
