#!/usr/bin/env bash
# Stop only api/matcher processes whose executable lives in this repo's
# target/debug directory. No sweep-kill: unrelated processes are left alone.
# On Linux, `make roadmap` checks start their own services; no Windows process
# cleanup is needed. The services-up target there is a no-op wrapper.
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) ;;
  *) echo "services-down: no-op on non-Windows"; exit 0 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)/target/debug"
root_win="$(cygpath -w "$root" 2>/dev/null || echo "$root")"

# PowerShell reads this from a file so MSYS cannot mangle $ variables.
ps1="$(dirname "$0")/.stop-services.ps1"
cat > "$ps1" <<PS
\$root = '$root_win'
Get-Process api,matcher -ErrorAction SilentlyContinue |
  Where-Object { try { \$_.Path -and \$_.Path.StartsWith(\$root) } catch { \$false } } |
  Stop-Process -Force -ErrorAction SilentlyContinue
PS
powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$ps1")" >/dev/null 2>&1 || true
echo "services-down: stopped project api/matcher (if any)"
