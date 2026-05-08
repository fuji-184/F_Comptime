#!/bin/bash
cargo test --features=rustime -- --no-capture
clear
cargo run