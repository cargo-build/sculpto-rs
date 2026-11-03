default:
    @just --list

build:
    cargo build -j 2

test:
    cargo test -j 2

fmt-check:
    cargo fmt --check

clippy:
    cargo clippy --all-targets -j 2 -- -D warnings

[positional-arguments]
test-real home *options:
    SCULPTO_BODYSLIDE_HOME="{{home}}" cargo test -p sculpto-core --test real_data {{options}}

gate: fmt-check clippy test
