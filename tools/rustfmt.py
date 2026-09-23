"""Format a generated Rust file the way `cargo fmt` formats the crate it is in,
so a regenerated table matches the committed, formatted one byte for byte and
`mise run fmt-check` stays clean. Every crate here is edition 2021 with default
rustfmt settings.
"""
import pathlib, subprocess


def rustfmt(path):
    try:
        subprocess.run(["rustfmt", "--edition", "2021", str(pathlib.Path(path))], check=True)
    except FileNotFoundError:
        raise SystemExit("rustfmt is not on PATH; run the generator under mise "
                         "(mise exec -- python ...) so the Rust toolchain is available")
