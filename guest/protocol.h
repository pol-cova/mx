#ifndef MX_PROTOCOL_H
#define MX_PROTOCOL_H

#include <stdint.h>

#define MX_PROTOCOL_VERSION 2
#define MX_MAX_FRAME (16u * 1024u * 1024u)
#define MX_IDLE_SECONDS 300
#define MX_SOCKET_PATH "/tmp/mx-guest.sock"

/* Snapshot request: method=snapshot, optional pid, want_tree, if_hash_not.
 * v2: if if_hash_not equals the current hash, omit elements and set unchanged=true.
 * want_tree=false omits elements and returns pid/width/height/hash/truncated.
 * v1 guests ignore these fields and always return elements. */

#endif
