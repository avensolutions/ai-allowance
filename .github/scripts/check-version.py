import re
import subprocess
import sys
import tomllib


def git(*args):
    return subprocess.check_output(["git", "--no-pager", *args])


def package_version(ref):
    manifest = tomllib.loads(git("show", f"{ref}:Cargo.toml").decode("utf-8"))
    version = manifest["package"]["version"]
    if not re.fullmatch(r"(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*)){2}", version):
        sys.exit(f"Expected a three-part Cargo package version, got {version!r}.")
    return version, tuple(map(int, version.split(".")))


def main(base, head):
    changes = git(
        "diff", "--no-renames", "--name-only", "-z", base, head, "--",
        "*.rs", ":(exclude).github/**", ":(exclude)docs/**",
    )
    if not changes:
        print("No Rust source changes; no version bump required.")
        return

    old_version, old_parts = package_version(base)
    new_version, new_parts = package_version(head)
    if new_parts <= old_parts:
        sys.exit(
            f"Rust changes require a Cargo.toml version higher than {old_version}; "
            f"found {new_version}."
        )
    print(f"Cargo package version increased: {old_version} to {new_version}.")


if __name__ == "__main__":
    main(*sys.argv[1:])
