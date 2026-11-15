#!/usr/bin/env bash
# Build sigiro for Android arm64 (aarch64-linux-android, API 30+).
#
# DuckDB publishes no Android static libs or android_arm64 extensions, so this
# builds DuckDB v2.0.0 from source with parquet/json/icu/ducklake linked in,
# merges every fragment into one libduckdb_static.a, and links sigiro against it.
#
# Needs: Android NDK (ANDROID_NDK_HOME), cmake, ninja, cargo-ndk,
#        rustup target aarch64-linux-android.
# After replacing libduckdb_static.a, run `cargo clean -p libduckdb-sys --target
# aarch64-linux-android --release --target-dir "$work/target"`: the archive is bundled
# into that rlib.
# Output: stripped binary at $work/sigiro-android-arm64
#         ($work defaults to ~/Library/Caches/sigiro/android-build).
set -euo pipefail

: "${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME to an NDK (tested with r30)}"
api=30
duckdb_version=v2.0.0
duckdb_commit=d54cbbe3df17e8e5ce68312f483b4155836112aa
croaring_version=v4.3.11
work="${SIGIRO_ANDROID_WORK:-$HOME/Library/Caches/sigiro/android-build}"
out="${HOME}/Library/Caches/sigiro/duckdb-static/${duckdb_version}/aarch64-linux-android"
[[ "$(uname -s)" == Darwin ]] || out="${XDG_CACHE_HOME:-$HOME/.cache}/sigiro/duckdb-static/${duckdb_version}/aarch64-linux-android"
bin="$(echo "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/bin)"
toolchain=(-DCMAKE_TOOLCHAIN_FILE="$ANDROID_NDK_HOME/build/cmake/android.toolchain.cmake"
  -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM="android-$api" -DCMAKE_BUILD_TYPE=Release -G Ninja)
mkdir -p "$work" "$out"

if [[ ! -s "$out/libduckdb_static.a" ]]; then
  # CRoaring: ducklake's deletion vectors need it (upstream gets it from vcpkg).
  if [[ ! -f "$work/roaring/lib/libroaring.a" ]]; then
    rm -rf "$work/croaring" && mkdir -p "$work/croaring"
    curl -fsSL "https://github.com/RoaringBitmap/CRoaring/archive/refs/tags/${croaring_version}.tar.gz" |
      tar -xz -C "$work/croaring" --strip-components 1
    cmake -S "$work/croaring" -B "$work/croaring/build" "${toolchain[@]}" \
      -DENABLE_ROARING_TESTS=OFF -DROARING_BUILD_STATIC=ON -DCMAKE_INSTALL_PREFIX="$work/roaring"
    ninja -C "$work/croaring/build" install
  fi

  src="$work/duckdb"
  if [[ ! -f "$src/CMakeLists.txt" ]]; then
    mkdir -p "$src"
    curl -fsSL "https://github.com/duckdb/duckdb/archive/refs/tags/${duckdb_version}.tar.gz" |
      tar -xz -C "$src" --strip-components 1
  fi
  # The version must read exactly v2.0.0: sigiro refuses a mismatched DuckDB.
  # DUCKDB_PLATFORM is explicit because the build can't run its detector binary.
  cmake -S "$src" -B "$src/build-android" "${toolchain[@]}" \
    -DBUILD_EXTENSIONS="parquet;json;icu" \
    -DDUCKDB_EXTENSION_CONFIGS="$src/.github/config/extensions/ducklake.cmake" \
    -Droaring_DIR="$work/roaring/lib/cmake/roaring" -DCMAKE_FIND_ROOT_PATH="$work/roaring" \
    -DBUILD_SHELL=0 -DBUILD_UNITTESTS=0 \
    -DENABLE_EXTENSION_AUTOLOADING=0 -DENABLE_EXTENSION_AUTOINSTALL=0 \
    -DDUCKDB_EXPLICIT_PLATFORM=android_arm64 -DDUCKDB_PLATFORM=android_arm64 \
    -DOVERRIDE_GIT_DESCRIBE="$duckdb_version" -DGIT_COMMIT_HASH="$duckdb_commit"
  ninja -C "$src/build-android"

  tmp="$out/libduckdb_static.a.tmp"
  rm -f "$tmp"
  {
    echo "create $tmp"
    (cd "$src/build-android" && find "$PWD" -name '*.a' ! -path '*/tools/*' ! -path '*/_deps/*' | sort | sed 's/^/addlib /')
    echo "addlib $work/roaring/lib/libroaring.a"
    echo save
    echo end
  } | "$bin/llvm-ar" -M
  "$bin/llvm-ranlib" "$tmp"
  cp "$src/src/include/duckdb.h" "$out/duckdb.h"
  mv "$tmp" "$out/libduckdb_static.a"
fi

export DUCKDB_LIB_DIR="$out" DUCKDB_INCLUDE_DIR="$out" DUCKDB_STATIC=1
# DuckDB is C++: link the NDK's static libc++ so the binary needs only bionic.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-lc++_static -C link-arg=-lc++abi"
# Own target dir: managed/shared target caches may not keep cross-built binaries around.
export CARGO_TARGET_DIR="$work/target"
cargo ndk -t arm64-v8a -P "$api" build --release --bin sigiro
"$bin/llvm-strip" -o "$work/sigiro-android-arm64" "$CARGO_TARGET_DIR/aarch64-linux-android/release/sigiro"
ls -la "$work/sigiro-android-arm64"
