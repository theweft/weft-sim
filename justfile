# Run `just` to list recipes. CI runs `just ci` inside `nix develop`.

out := "target/guest"
musl := env("WEFT_GUEST_ARCH", "x86_64") + "-unknown-linux-musl"

default:
    @just --list

# Format all code and TOML.
fmt:
    cargo fmt --all
    taplo fmt

# Check formatting without changing files.
fmt-check:
    cargo fmt --all --check
    taplo fmt --check

# Lints; warnings are errors.
clippy:
    cargo clippy --locked --all-targets -- -D warnings

# Unit tests (wire format, request handling).
test:
    cargo test --locked

# Kernel module against the guest kernel.
module:
    make -C "$WEFT_KERNEL_BUILD" M="$PWD/module" W=1 modules

# Static runner for the guest.
runner:
    cargo build --locked --release --target {{musl}}

# Overlays and initramfs.
guest: module runner
    mkdir -p {{out}}
    dtc -q -I dts -O dtb -o {{out}}/base.dtbo guest/base.dts
    dtc -q -@ -I dts -O dtb -o {{out}}/board.dtbo tests/boards/ads1115.dts
    BUSYBOX="$WEFT_BUSYBOX" MODULES="$WEFT_KERNEL_MODULES" \
    RUNNER=target/{{musl}}/release/weft-sim SIM_KO=module/weft_sim.ko \
    BASE={{out}}/base.dtbo BOARD={{out}}/board.dtbo OUT={{out}}/initramfs.gz sh guest/build.sh

# Boot the guest and run the driver test. `just sim weft.trace` logs every transfer.
sim *args: guest
    #!/bin/sh
    set -eu
    accel=tcg; [ -w /dev/kvm ] && accel=kvm
    echo "accelerator: $accel"
    log={{out}}/boot.log
    timeout 120 qemu-system-"$WEFT_GUEST_ARCH" -accel "$accel" -cpu max -m 256 -nographic -no-reboot \
        -kernel "$WEFT_KERNEL_IMAGE" -initrd {{out}}/initramfs.gz \
        -append "console=ttyS0 quiet panic=-1 {{args}}" >"$log" 2>&1 || true
    grep -a -E "^(SIM|weft-sim)" "$log" | tr -d '\r'
    grep -a -q "SIM PASS" "$log"

# Licences, advisories, banned crates, allowed sources.
deny:
    cargo deny check

# Spelling.
typos:
    typos

# Licence and copyright: REUSE compliance, plus an SPDX header in every Rust file.
licenses:
    reuse lint
    @missing=$(git ls-files '*.rs' | while read -r f; do head -2 "$f" | grep -q 'SPDX-''License-Identifier: MIT' || echo "$f"; done); \
    if [ -n "$missing" ]; then echo "missing SPDX header:"; echo "$missing"; exit 1; fi

# API docs; broken links are errors.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps

# Everything CI checks.
ci: fmt-check clippy test deny licenses typos doc sim
