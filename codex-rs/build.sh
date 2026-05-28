#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  ./build.sh --target mac|linux [options] [-- <extra cargo args>]

Builds codex-cli with the local lean feature set and packages the binary with
the bundled patched zsh under the layout Codex expects:

  <package>/bin/codex
  <package>/codex-resources/zsh/bin/zsh
  <package>/codex-package.json

Options:
  --target mac|linux       Required. Selects the feature set and package name.
  --profile NAME           Cargo profile to build. Defaults to lto.
  -j, --jobs N             Cargo jobs. Defaults to nproc/sysctl hw.ncpu.
  --rust-target TRIPLE     Optional Cargo target triple for cross builds.
  --out-dir DIR            Package output directory. Defaults to target/packages.
  --skip-build             Do not run cargo; package the existing target binary.
  --codex-bin FILE         Package this prebuilt codex binary; implies --skip-build.
  --zsh-dir DIR            Directory containing bin/zsh, zsh/bin/zsh, or a zsh file.
  --zsh-tar FILE           Tarball containing codex-zsh/bin/zsh, zsh/bin/zsh, or bin/zsh.
  -h, --help               Show this help.

Defaults:
  cargo build -p codex-cli --no-default-features --profile lto

The script does not build zsh. It packages an existing patched zsh from
--zsh-dir, --zsh-tar, CODEX_ZSH_DIR, CODEX_ZSH_TARBALL, or ./codex-zsh.
EOF
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$repo_root"

target_mode=""
profile="lto"
jobs=""
rust_target=""
out_dir="target/packages"
skip_build=0
codex_bin=""
zsh_dir="${CODEX_ZSH_DIR:-}"
zsh_tar="${CODEX_ZSH_TARBALL:-}"
extra_cargo_args=()

while (($#)); do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || die "--target requires mac or linux"
      target_mode="$2"
      shift 2
      ;;
    --profile)
      [[ $# -ge 2 ]] || die "--profile requires a value"
      profile="$2"
      shift 2
      ;;
    -j|--jobs)
      [[ $# -ge 2 ]] || die "$1 requires a value"
      jobs="$2"
      shift 2
      ;;
    --rust-target)
      [[ $# -ge 2 ]] || die "--rust-target requires a target triple"
      rust_target="$2"
      shift 2
      ;;
    --out-dir)
      [[ $# -ge 2 ]] || die "--out-dir requires a path"
      out_dir="$2"
      shift 2
      ;;
    --skip-build)
      skip_build=1
      shift
      ;;
    --codex-bin)
      [[ $# -ge 2 ]] || die "--codex-bin requires a path"
      codex_bin="$2"
      skip_build=1
      shift 2
      ;;
    --zsh-dir)
      [[ $# -ge 2 ]] || die "--zsh-dir requires a path"
      zsh_dir="$2"
      shift 2
      ;;
    --zsh-tar)
      [[ $# -ge 2 ]] || die "--zsh-tar requires a path"
      zsh_tar="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      extra_cargo_args=("$@")
      break
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

case "$target_mode" in
  mac|linux) ;;
  "") die "--target mac or --target linux is required" ;;
  *) die "--target must be mac or linux, got: $target_mode" ;;
esac

if [[ -z "$jobs" ]]; then
  if command -v nproc >/dev/null 2>&1; then
    jobs="$(nproc)"
  elif command -v sysctl >/dev/null 2>&1; then
    jobs="$(sysctl -n hw.ncpu)"
  else
    jobs="4"
  fi
fi

host_triple="$(rustc -vV | awk '/^host:/ { print $2 }')"
[[ -n "$host_triple" ]] || die "failed to determine rustc host triple"

if [[ -z "$rust_target" ]]; then
  case "$target_mode:$host_triple" in
    mac:*apple-darwin*) ;;
    linux:*linux*) ;;
    mac:*) die "--target mac must run on macOS unless --rust-target is provided" ;;
    linux:*) die "--target linux must run on Linux unless --rust-target is provided" ;;
  esac
fi

cargo_target_args=()
target_triple="$host_triple"
if [[ -n "$rust_target" ]]; then
  cargo_target_args=(--target "$rust_target")
  target_triple="$rust_target"
fi

feature_args=()
case "$target_mode" in
  mac)
    feature_args=()
    ;;
  linux)
    feature_args=()
    ;;
esac

if [[ -n "$codex_bin" ]]; then
  built_bin="$codex_bin"
elif [[ -n "$rust_target" ]]; then
  built_bin="target/$rust_target/$profile/codex"
else
  built_bin="target/$profile/codex"
fi

if ((skip_build)); then
  ((${#extra_cargo_args[@]} == 0)) || die "extra cargo args cannot be used with --skip-build"
  printf "Packaging existing codex binary for %s (%s): %s\n" "$target_mode" "$target_triple" "$built_bin"
else
  printf "Building codex-cli for %s (%s)\n" "$target_mode" "$target_triple"
  cargo build \
    -p codex-cli \
    --no-default-features \
    "${feature_args[@]}" \
    --profile "$profile" \
    -j "$jobs" \
    "${cargo_target_args[@]}" \
    "${extra_cargo_args[@]}"
fi

[[ -x "$built_bin" ]] || die "built binary not found or not executable: $built_bin"

if command -v file >/dev/null 2>&1; then
  codex_file_type="$(file -b "$built_bin")"
  case "$target_mode:$codex_file_type" in
    mac:*Mach-O*) ;;
    linux:*ELF*) ;;
    *) die "codex binary at $built_bin does not look like a $target_mode binary: $codex_file_type" ;;
  esac
fi

tmp_dir=""
cleanup() {
  if [[ -n "$tmp_dir" ]]; then
    rm -rf "$tmp_dir"
  fi
}
trap cleanup EXIT

resolve_zsh() {
  local candidate

  if [[ -n "$zsh_dir" ]]; then
    for candidate in "$zsh_dir/bin/zsh" "$zsh_dir/zsh/bin/zsh" "$zsh_dir/zsh"; do
      if [[ -x "$candidate" ]]; then
        printf '%s\n' "$candidate"
        return 0
      fi
    done
    die "no executable zsh found in --zsh-dir: $zsh_dir"
  fi

  if [[ -n "$zsh_tar" ]]; then
    [[ -f "$zsh_tar" ]] || die "zsh tarball not found: $zsh_tar"
  elif [[ -f "$repo_root/codex-zsh-$target_triple.tar.gz" ]]; then
    zsh_tar="$repo_root/codex-zsh-$target_triple.tar.gz"
  elif [[ "$target_mode" == "linux" && -f "$repo_root/codex-zsh-x86_64-unknown-linux-musl.tar.gz" ]]; then
    zsh_tar="$repo_root/codex-zsh-x86_64-unknown-linux-musl.tar.gz"
  elif [[ -x "$repo_root/codex-zsh/bin/zsh" ]]; then
    printf "%s\n" "$repo_root/codex-zsh/bin/zsh"
    return 0
  else
    die "no bundled zsh found; pass --zsh-dir or --zsh-tar"
  fi

  tmp_dir="$(mktemp -d)"
  tar -xzf "$zsh_tar" -C "$tmp_dir"
  for candidate in "$tmp_dir/codex-zsh/bin/zsh" "$tmp_dir/zsh/bin/zsh" "$tmp_dir/bin/zsh"; do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  die "zsh tarball does not contain codex-zsh/bin/zsh, zsh/bin/zsh, or bin/zsh: $zsh_tar"
}

zsh_path="$(resolve_zsh)"
if command -v file >/dev/null 2>&1; then
  zsh_file_type="$(file -b "$zsh_path")"
  case "$target_mode:$zsh_file_type" in
    mac:*Mach-O*) ;;
    linux:*ELF*) ;;
    *) die "zsh at $zsh_path does not look like a $target_mode binary: $zsh_file_type" ;;
  esac
fi

package_name="codex-$target_mode-$target_triple-$profile"
package_dir="$out_dir/$package_name"
resources_zsh_dir="$package_dir/codex-resources/zsh/bin"

mkdir -p "$package_dir/bin" "$resources_zsh_dir"
cp "$built_bin" "$package_dir/bin/codex"
cp "$zsh_path" "$resources_zsh_dir/zsh"
chmod 0755 "$package_dir/bin/codex" "$resources_zsh_dir/zsh"
printf '{}\n' >"$package_dir/codex-package.json"

tarball="$out_dir/$package_name.tar.gz"
mkdir -p "$out_dir"
tar -C "$out_dir" -czf "$tarball" "$package_name"

printf 'Packaged: %s\n' "$package_dir"
printf 'Tarball:  %s\n' "$tarball"
