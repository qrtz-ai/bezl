#!/usr/bin/env bash
# ADR 0141: names Linux can't check. On macOS XCTestCase is an NSObject, so inside a test class an unqualified call
# resolves to an inherited member before a file-scope function (49f1fed: a private `load()` was shadowed by
# NSObject's `+load`), and a member that reuses an NSObject / XCTestCase name needs `override` or clashes with an
# Objective-C selector. Linux's XCTestCase is a plain Swift class, so neither shows up there. This fails if
#   - a file-scope func / let / var in Tests/QrtzCtrlTests or Sources/QrtzCtrl takes one of those names, or
#   - a member of an XCTestCase subclass (4-space indent under `class X: XCTestCase`) takes one without `override`.
# Usage: scripts/ctrl/objc-names.sh   (bash + awk; run by scripts/ctrl/mac-typecheck.sh)
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
names="load initialize description debugDescription hash copy mutableCopy isEqual dealloc finalize superclass isProxy
isKind isMember responds conforms perform value setValue observeValue className classForCoder zone retain release
autorelease retainCount name run measure expectation wait record invokeTest setUp tearDown addTeardownBlock testRun
invocation continueAfterFailure executionTimeAllowance defaultTestSuite keyValueObservingExpectation"
found=$(find "$root/native/qrtz-helper/Tests/QrtzCtrlTests" "$root/native/qrtz-helper/Sources/QrtzCtrl" -name '*.swift' -print0 |
  xargs -0 awk -v names="$names" '
    BEGIN { n = split(names, a, /[ \n]+/); for (i = 1; i <= n; i++) if (a[i] != "") bad[a[i]] = 1 }
    FNR == 1 { inTest = 0 }
    function declName(line,   m) {
      if (match(line, /(func|var|let) [A-Za-z_][A-Za-z0-9_]*/)) { m = substr(line, RSTART, RLENGTH); sub(/^(func|var|let) /, "", m); return m }
      return ""
    }
    /^((final|private|fileprivate|public|internal|open) )*class [A-Za-z0-9_]+ *: *XCTestCase/ { inTest = 1; next }
    /^}/ { inTest = 0; next }
    /^((private|fileprivate|internal|public|static) )*(func|var|let) / {
      d = declName($0); if (d in bad) print FILENAME ":" FNR ": file-scope `" d "` (an NSObject / XCTestCase member on macOS)"; next
    }
    inTest && /^    ((private|fileprivate|internal|public|static|class|final|@objc|lazy) )*(func|var|let) / {
      d = declName($0); if ((d in bad) && $0 !~ /override/) print FILENAME ":" FNR ": XCTestCase member `" d "` without override (NSObject / XCTestCase name on macOS)"
    }')
if [ -n "$found" ]; then
  echo "$found"
  echo "rename these: on macOS they collide with NSObject / XCTestCase members (ADR 0141)" >&2
  exit 1
fi
echo "no NSObject / XCTestCase name collisions in QrtzCtrl sources or tests"
