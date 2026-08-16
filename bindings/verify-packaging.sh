#!/usr/bin/env bash
# Proof: installed npm/pip/CLI packages open a file and SELECT schema_version.
# Does not copy a dylib by hand. Does not publish.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export AIDB_NATIVE_PROFILE="${AIDB_NATIVE_PROFILE:-debug}"

echo "==> build natives + CLI"
cargo build -p aidb-node -p aidb-python -p aidb-cli

echo "==> stage into packages"
node bindings/typescript/scripts/stage-native.mjs
python3 bindings/python/scripts/stage_native.py

TMP="$(mktemp -d "${TMPDIR:-/tmp}/aidb-packaging.XXXXXX")"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

SQL="SELECT value FROM aidb_meta WHERE key = 'schema_version'"

echo "==> CLI binary"
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  AIDB_BIN="$CARGO_TARGET_DIR/$AIDB_NATIVE_PROFILE/aidb"
else
  AIDB_BIN="$ROOT/target/$AIDB_NATIVE_PROFILE/aidb"
fi
VERSION="$("$AIDB_BIN" sql "$TMP/cli.db" "$SQL" | tail -n 1 | tr -d '[:space:]')"
if [[ ! "$VERSION" =~ ^[0-9]+$ ]]; then
  echo "cli schema_version: $VERSION" >&2
  exit 1
fi
echo "cli ok schema_version=$VERSION"

echo "==> npm pack + install"
PACK_DIR="$TMP/npm-pack"
mkdir -p "$PACK_DIR"
(cd bindings/typescript && npm pack --ignore-scripts --pack-destination "$PACK_DIR")
TGZ="$(ls "$PACK_DIR"/aidb-*.tgz | head -n 1)"
NPM_APP="$TMP/npm-app"
mkdir -p "$NPM_APP"
(
  cd "$NPM_APP"
  npm init -y >/dev/null
  npm i --ignore-scripts "$TGZ"
  node --input-type=module -e "
import { AI } from 'aidb';
const db = await AI.open('$TMP/npm.db');
const v = await db.query(\"$SQL\");
if (!/^\d+$/.test(String(v.rows[0][0]))) throw new Error(JSON.stringify(v));
await db.close();
console.log('npm ok schema_version=' + v.rows[0][0]);
"
)

echo "==> pip install"
python3 -m venv "$TMP/venv"
"$TMP/venv/bin/python" -m pip install -q --upgrade pip setuptools wheel
"$TMP/venv/bin/python" -m pip install -q --no-build-isolation "$ROOT/bindings/python"
"$TMP/venv/bin/python" -c "
from aidb import AI
db = AI.open(r'$TMP/pip.db')
v = db.query(\"SELECT value FROM aidb_meta WHERE key = 'schema_version'\")
ver = str(v['rows'][0][0])
assert ver.isdigit() and int(ver) >= 5, v
db.close()
print('pip ok schema_version=' + ver)
"

echo "packaging ok"
