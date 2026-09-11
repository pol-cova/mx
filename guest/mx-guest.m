#import "protocol.h"

#include <stdio.h>
#include <string.h>

int MXServe(const char *socketPath);

int main(int argc, char **argv) {
    const char *path = NULL;
    if (argc >= 2 && strcmp(argv[1], "serve") == 0) {
        if (argc >= 4 && strcmp(argv[2], "--socket") == 0) {
            path = argv[3];
        }
        return MXServe(path);
    }
    fprintf(stderr, "usage: mx-guest serve [--socket PATH]\n");
    return 2;
}
