// qrtz dev launcher (macOS): exec the given program as its own TCC "responsible process".
//
// A program started from Terminal, an IDE or a remote shell normally inherits its launcher as the responsible process,
// so macOS checks Accessibility, Screen Recording, Microphone and Automation against the TERMINAL, not qrtz-app, and a
// grant given to qrtz-app is ignored. Disclaiming responsibility (what Xcode/LLDB do for apps they run) makes qrtz-app
// responsible for itself, so its own stable signature (scripts/dev/sign.sh) is what TCC checks, and the grant survives
// rebuilds. POSIX_SPAWN_SETEXEC replaces this process in place (same pid), so `tauri dev` still kills the app itself.
// If the private symbol is missing, it falls back to a plain exec.
#include <dlfcn.h>
#include <errno.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

extern char **environ;

int main(int argc, char **argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: qrtz-dev-launch <program> [args...]\n");
    return 64;
  }
  int (*disclaim)(posix_spawnattr_t *, int) =
      (int (*)(posix_spawnattr_t *, int))dlsym(RTLD_DEFAULT, "responsibility_spawnattrs_setdisclaim");
  if (disclaim) {
    posix_spawnattr_t attr;
    if (posix_spawnattr_init(&attr) == 0 && disclaim(&attr, 1) == 0 &&
        posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETEXEC) == 0) {
      int r = posix_spawn(NULL, argv[1], NULL, &attr, &argv[1], environ);
      fprintf(stderr, "qrtz-dev-launch: disclaimed spawn of %s failed (%s), plain exec\n", argv[1], strerror(r));
    }
  }
  execv(argv[1], &argv[1]);
  fprintf(stderr, "qrtz-dev-launch: exec %s: %s\n", argv[1], strerror(errno));
  return 127;
}
