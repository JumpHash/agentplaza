/* profile-exit.so: a SIGTERM ends the process through exit(), so its atexit handlers run. coverage.sh preloads
 * it into the instrumented hub: apid has no signal handler, and an LLVM profile is written at exit only. */
#include <signal.h>
#include <stdlib.h>

static void on_term(int sig) {
    (void)sig;
    exit(128 + SIGTERM);
}

__attribute__((constructor)) static void install(void) {
    signal(SIGTERM, on_term);
}
