#!/usr/bin/env bash
#
# NeuxDOT build driver.
#
#   ./compile.sh --server             neuxdot-server (static musl) -> release/
#   ./compile.sh --linux              desktop GUI binary        -> release/
#   ./compile.sh --plugins            built-in plugins (computer, chat, gitlab) -> release/plugins/
#   ./compile.sh --android            signed release APK        -> release/
#   ./compile.sh --server --android   any combination
#   ./compile.sh --all                all components
#
# Artifacts are written to ./release/.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$ROOT/release"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)"/\1/')"

DO_SERVER=0
DO_LINUX=0
DO_PLUGINS=0
DO_ANDROID=0

usage() { sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

if [ $# -eq 0 ]; then usage; exit 1; fi

for arg in "$@"; do
  case "$arg" in
    --server)  DO_SERVER=1 ;;
    --linux)   DO_LINUX=1 ;;
    --plugins) DO_PLUGINS=1 ;;
    --android) DO_ANDROID=1 ;;
    --all)     DO_SERVER=1; DO_LINUX=1; DO_PLUGINS=1; DO_ANDROID=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown flag: $arg" >&2; usage; exit 1 ;;
  esac
done

# Some environments mount the default cargo/rustup/npm/gradle caches read-only.
# Prefer them if writable, otherwise fall back to a scratch dir.
writable() { [ -d "$1" ] && [ -w "$1" ]; }
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
if ! writable "$CARGO_HOME/registry"; then
  export CARGO_HOME="${CARGO_HOME_OVERRIDE:-/tmp/cargo-home}"
  mkdir -p "$CARGO_HOME"
fi
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$HOME/.gradle}"
if ! writable "$GRADLE_USER_HOME"; then
  export GRADLE_USER_HOME="${GRADLE_USER_HOME_OVERRIDE:-/tmp/gradle-home}"
  mkdir -p "$GRADLE_USER_HOME"
fi

mkdir -p "$OUT"

say() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m    %s\033[0m\n' "$*"; }

# ---------------------------------------------------------------- server
build_server() {
  say "Building server (static musl)"
  if command -v musl-gcc >/dev/null 2>&1; then
    cargo build --release -p neuxdot-server --target x86_64-unknown-linux-musl
    cp "$ROOT/target/x86_64-unknown-linux-musl/release/neuxdot-server" "$OUT/neuxdot-server"
  elif command -v docker >/dev/null 2>&1; then
    # No local musl toolchain: build in a container so the output is static.
    docker run --rm -v "$ROOT":/src -v "$CARGO_HOME":/cargo-home -w /src \
      -e CARGO_HOME=/cargo-home rust:1-alpine \
      sh -c "trap 'chown -R $(id -u):$(id -g) /src/target /cargo-home 2>/dev/null || true' EXIT; apk add --no-cache musl-dev gcc >/dev/null 2>&1 && cargo build --release -p neuxdot-server --target x86_64-unknown-linux-musl"
    cp "$ROOT/target/x86_64-unknown-linux-musl/release/neuxdot-server" "$OUT/neuxdot-server"
  else
    echo "    no musl toolchain and no docker; building glibc binary instead" >&2
    cargo build --release -p neuxdot-server
    cp "$ROOT/target/release/neuxdot-server" "$OUT/neuxdot-server"
  fi
  strip "$OUT/neuxdot-server" 2>/dev/null || true
  ok "release/neuxdot-server ($(du -h "$OUT/neuxdot-server" | cut -f1))"
}

# ----------------------------------------------------------------- linux
build_linux() {
  say "Building desktop (Linux)"

  # The desktop is a web client: it must embed the built frontend.
  #
  # `cargo build -p neuxdot-desktop` alone compiles the shell but leaves
  # `devUrl` active, so the release binary loads http://localhost:5173 and the
  # window shows "Could not connect to localhost: Connection refused". A
  # production build has to ship the static assets, which means building the web
  # app first and pointing tauri at it with `devUrl` disabled.
  local web_dir="$ROOT/src/apps/web"
  local dist="$web_dir/build"

  say "Building web frontend (static)"
  # Resolve vite in this order:
  #   1. the workspace .bin shim (normal case)
  #   2. the real package inside the root virtual store — the shims generated
  #      before the src/ restructure point at src/node_modules/.pnpm and break
  #   3. pnpm, if available
  local vite_js=""
  if ( cd "$web_dir" && ./node_modules/.bin/vite --version >/dev/null 2>&1 ); then
    vite_js="bin"   # call the shim directly
  else
    vite_js="$(ls "$ROOT"/node_modules/.pnpm/vite@*/node_modules/vite/bin/vite.js 2>/dev/null | head -1)"
  fi

  if [ "$vite_js" = "bin" ]; then
    ( cd "$web_dir" && ./node_modules/.bin/vite build )
  elif [ -n "$vite_js" ]; then
    ( cd "$web_dir" && node "$vite_js" build )
  elif command -v pnpm >/dev/null 2>&1; then
    ( cd "$ROOT/config" && pnpm --filter web build )
  else
    echo "    cannot build the web frontend (no working vite, no pnpm)." >&2
    echo "    Reinstall deps: cd config && pnpm install" >&2
    exit 1
  fi
  [ -f "$dist/index.html" ] || { echo "    web build missing $dist/index.html" >&2; exit 1; }
  ok "frontend built ($dist)"

  # Override the tauri config for this build only: disable the dev server and
  # embed the dist directory (absolute, so it does not depend on CWD).
  local tauri_config
  tauri_config="$(printf '{"build":{"devUrl":null,"frontendDist":"%s"}}' "$dist")"

  TAURI_CONFIG="$tauri_config" cargo build --release -p neuxdot-desktop
  cp "$ROOT/target/release/neuxdot-desktop" "$OUT/neuxdot-desktop"
  strip "$OUT/neuxdot-desktop" 2>/dev/null || true

  # Guard: a dev-mode binary would silently reproduce the connection error.
  if strings "$OUT/neuxdot-desktop" 2>/dev/null | grep -q "localhost:5173"; then
    echo "    build still references the dev server; frontend was not embedded" >&2
    exit 1
  fi

  ok "release/neuxdot-desktop ($(du -h "$OUT/neuxdot-desktop" | cut -f1))"
  echo "    note: links GTK3 + WebKitGTK 4.1; not static (WebKit cannot be)."
}

# -------------------------------------------------------------- plugins
build_plugins() {
  say "Building built-in plugins (computer, chat & gitlab stdio MCP)"
  mkdir -p "$OUT/plugins"

  # 1. Computer MCP
  local comp_dir="$ROOT/built-in-plugin/computer"
  cargo build --release --manifest-path "$comp_dir/Cargo.toml"
  cp "$comp_dir/target/release/computer" "$OUT/plugins/computer"
  strip "$OUT/plugins/computer" 2>/dev/null || true
  ok "release/plugins/computer ($(du -h "$OUT/plugins/computer" | cut -f1))"

  # 2. Chat MCP
  local chat_dir="$ROOT/built-in-plugin/chat"
  cargo build --release --manifest-path "$chat_dir/Cargo.toml"
  cp "$chat_dir/target/release/chat" "$OUT/plugins/chat"
  strip "$OUT/plugins/chat" 2>/dev/null || true
  ok "release/plugins/chat ($(du -h "$OUT/plugins/chat" | cut -f1))"

  # 3. GitLab MCP
  local gitlab_dir="$ROOT/built-in-plugin/gitlab"
  cargo build --release --manifest-path "$gitlab_dir/Cargo.toml"
  cp "$gitlab_dir/target/release/gitlab" "$OUT/plugins/gitlab"
  strip "$OUT/plugins/gitlab" 2>/dev/null || true
  ok "release/plugins/gitlab ($(du -h "$OUT/plugins/gitlab" | cut -f1))"
}

# --------------------------------------------------------------- android
build_android() {
  say "Building Android APK (arm64, self-signed)"

  # `cargo tauri` is a cargo *subcommand*: it is provided by a `cargo-tauri`
  # binary on PATH. Check multiple resolutions so cargo, local node_modules,
  # or npx/pnpm work out of the box.
  local tauri_bin=""
  if command -v cargo-tauri >/dev/null 2>&1; then
    tauri_bin="cargo-tauri"
  elif cargo tauri --version >/dev/null 2>&1; then
    tauri_bin="cargo tauri"
  elif [ -x "$ROOT/src/apps/web/node_modules/.bin/tauri" ]; then
    tauri_bin="$ROOT/src/apps/web/node_modules/.bin/tauri"
  elif [ -x "$ROOT/node_modules/.bin/tauri" ]; then
    tauri_bin="$ROOT/node_modules/.bin/tauri"
  elif command -v npx >/dev/null 2>&1 && npx -y @tauri-apps/cli --version >/dev/null 2>&1; then
    tauri_bin="npx -y @tauri-apps/cli"
  elif command -v pnpm >/dev/null 2>&1 && pnpm dlx @tauri-apps/cli --version >/dev/null 2>&1; then
    tauri_bin="pnpm dlx @tauri-apps/cli"
  fi
  if [ -z "$tauri_bin" ]; then
    cat >&2 <<'EOS'
    tauri-cli not found. Android builds are driven by it. Install one of:
      cargo install tauri-cli --version '^2'   # needs crates.io access
      pnpm --dir src/apps/web add -D @tauri-apps/cli
    Nothing else was changed.
EOS
    exit 1
  fi
  if [ ! -f "$ROOT/.secrets/neuxdot-release.jks" ] && [ -f "$ROOT/config/.secrets/neuxdot-release.jks" ]; then
    ln -sfn "$ROOT/config/.secrets" "$ROOT/.secrets"
  fi
  if [ ! -f "$ROOT/.secrets/neuxdot-release.jks" ]; then
    echo "    missing .secrets/neuxdot-release.jks (release signing key)." >&2
    echo "    Generate one, then re-run:" >&2
    echo "      mkdir -p .secrets && keytool -genkeypair -v \\" >&2
    echo "        -keystore .secrets/neuxdot-release.jks -alias neuxdot \\" >&2
    echo "        -keyalg RSA -keysize 2048 -validity 10000 \\" >&2
    echo "        -storepass <pass> -keypass <pass> -dname 'CN=NeuxDOT'" >&2
    echo "    and record the passwords in .secrets/keystore.properties" >&2
    exit 1
  fi

  : "${ANDROID_HOME:=${HOME}/Android/Sdk}"
  : "${NDK_HOME:=$(ls -d "${ANDROID_HOME}"/ndk/* 2>/dev/null | sort -V | tail -1)}"
  export ANDROID_HOME NDK_HOME
  [ -n "${JAVA_HOME:-}" ] || export JAVA_HOME="$(dirname "$(dirname "$(readlink -f "$(command -v java)")")")"
  export PATH="$HOME/.local/bin:$ROOT/src/apps/web/node_modules/.bin:$PATH"

  # Run from the desktop crate: that directory holds tauri.conf.json, and
  # `cargo tauri` resolves the web app relative to it.
  local desktop_dir="$ROOT/src/apps/desktop"
  [ -f "$desktop_dir/tauri.conf.json" ] || {
    echo "    missing $desktop_dir/tauri.conf.json" >&2
    exit 1
  }

  mkdir -p "$desktop_dir/gen/android"
  cat << EOF > "$desktop_dir/gen/android/local.properties"
sdk.dir=$ANDROID_HOME
ndk.dir=$NDK_HOME
EOF
  ( cd "$desktop_dir" && $tauri_bin android build --apk --target aarch64 )

  APK="$(find "$desktop_dir/gen/android/app/build/outputs/apk" \( -name '*universal*release*.apk' -o -name '*arm64*release*.apk' -o -name '*release*.apk' \) 2>/dev/null | head -1)"
  [ -n "$APK" ] && [ -f "$APK" ] || { echo "    APK not found under $desktop_dir/gen/android" >&2; exit 1; }
  cp "$APK" "$OUT/neuxdot-${VERSION}-arm64.apk"
  ok "release/neuxdot-${VERSION}-arm64.apk ($(du -h "$OUT/neuxdot-${VERSION}-arm64.apk" | cut -f1))"
}

[ "$DO_SERVER"  = 1 ] && build_server
[ "$DO_LINUX"   = 1 ] && build_linux
[ "$DO_PLUGINS" = 1 ] && build_plugins
[ "$DO_ANDROID" = 1 ] && build_android

say "Done. Artifacts in release/"
ls -lh "$OUT"
