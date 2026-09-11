#import "ax.h"
#import "protocol.h"

#include <arpa/inet.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static void MXTune(int fd) {
    int on = 1;
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, sizeof(on));
}

static volatile sig_atomic_t MXRunning = 1;

static void MXStop(int signal) {
    (void)signal;
    MXRunning = 0;
    CFRunLoopStop(CFRunLoopGetMain());
}

static BOOL MXWriteFrame(int fd, NSData *body) {
    if (body.length > MX_MAX_FRAME) {
        return NO;
    }
    uint32_t size = htonl((uint32_t)body.length);
    if (write(fd, &size, sizeof(size)) != sizeof(size)) {
        return NO;
    }
    const uint8_t *bytes = body.bytes;
    NSUInteger remaining = body.length;
    while (remaining > 0) {
        ssize_t written = write(fd, bytes, remaining);
        if (written <= 0) {
            return NO;
        }
        bytes += written;
        remaining -= (NSUInteger)written;
    }
    return YES;
}

static NSData *MXReadFrame(int fd) {
    uint32_t size = 0;
    uint8_t *header = (uint8_t *)&size;
    NSUInteger got = 0;
    while (got < sizeof(size)) {
        ssize_t n = read(fd, header + got, sizeof(size) - got);
        if (n <= 0) {
            return nil;
        }
        got += (NSUInteger)n;
    }
    size = ntohl(size);
    if (size == 0 || size > MX_MAX_FRAME) {
        return nil;
    }
    NSMutableData *body = [NSMutableData dataWithLength:size];
    uint8_t *bytes = body.mutableBytes;
    NSUInteger remaining = size;
    while (remaining > 0) {
        ssize_t n = read(fd, bytes, remaining);
        if (n <= 0) {
            return nil;
        }
        bytes += n;
        remaining -= (NSUInteger)n;
    }
    return body;
}

static NSDictionary *MXReplyError(NSNumber *identifier, NSString *message) {
    return @{@"id": identifier ?: [NSNull null], @"ok": @NO, @"error": message};
}

static NSDictionary *MXOnMain(NSDictionary * (^block)(void)) {
    if ([NSThread isMainThread]) {
        return block();
    }
    __block NSDictionary *result = nil;
    dispatch_sync(dispatch_get_main_queue(), ^{
        result = block();
    });
    return result;
}

static NSDictionary *MXHandle(NSDictionary *request) {
    NSNumber *identifier = request[@"id"];
    NSString *method = request[@"method"];
    if ([method isEqual:@"hello"]) {
        return MXOnMain(^{
            (void)MXFrameworkStatus();
            return @{
                @"id": identifier ?: [NSNull null],
                @"ok": @YES,
                @"protocol": @(MX_PROTOCOL_VERSION),
                @"guest": @"mx-guest",
                @"xct": MXFrameworkStatus()
            };
        });
    }
    NSError *error = nil;
    if ([method isEqual:@"snapshot"]) {
        pid_t pid = [request[@"pid"] respondsToSelector:@selector(intValue)] ? [request[@"pid"] intValue] : 0;
        NSString *ifHashNot = [request[@"if_hash_not"] isKindOfClass:NSString.class] ? request[@"if_hash_not"] : nil;
        BOOL wantTree = YES;
        if (request[@"want_tree"] != nil && [request[@"want_tree"] respondsToSelector:@selector(boolValue)]) {
            wantTree = [request[@"want_tree"] boolValue];
        }
        NSDictionary *tree = MXSnapshot(pid, ifHashNot, wantTree, &error);
        if (!tree) {
            return MXReplyError(identifier, error.localizedDescription ?: @"snapshot failed");
        }
        NSMutableDictionary *reply = [tree mutableCopy];
        reply[@"id"] = identifier ?: [NSNull null];
        reply[@"ok"] = @YES;
        return (NSDictionary *)reply;
    }
    if ([method isEqual:@"press"]) {
        pid_t pid = [request[@"pid"] respondsToSelector:@selector(intValue)] ? [request[@"pid"] intValue] : 0;
        if (!MXPress(pid, request[@"identifier"], request[@"label"], request[@"role"], &error)) {
            return MXReplyError(identifier, error.localizedDescription ?: @"press failed");
        }
        return @{@"id": identifier ?: [NSNull null], @"ok": @YES};
    }
    if ([method isEqual:@"set_value"]) {
        if (!MXSetValue(request[@"text"] ?: @"", &error)) {
            return MXReplyError(identifier, error.localizedDescription ?: @"set_value failed");
        }
        return @{@"id": identifier ?: [NSNull null], @"ok": @YES};
    }
    return MXReplyError(identifier, @"unknown method");
}

static void MXServeClient(int fd) {
    while (MXRunning) {
        NSData *body = MXReadFrame(fd);
        if (!body) {
            break;
        }
        NSError *error = nil;
        id request = [NSJSONSerialization JSONObjectWithData:body options:0 error:&error];
        NSDictionary *reply = [request isKindOfClass:NSDictionary.class]
            ? MXHandle(request)
            : MXReplyError(nil, @"request must be a JSON object");
        NSData *encoded = [NSJSONSerialization dataWithJSONObject:reply options:0 error:nil];
        if (!encoded || !MXWriteFrame(fd, encoded)) {
            break;
        }
    }
    close(fd);
}

int MXServe(const char *socketPath) {
    const char *path = (socketPath && socketPath[0]) ? socketPath : MX_SOCKET_PATH;
    signal(SIGTERM, MXStop);
    signal(SIGINT, MXStop);
    unlink(path);
    int server = socket(AF_UNIX, SOCK_STREAM, 0);
    if (server < 0) {
        perror("socket");
        return 1;
    }
    struct sockaddr_un address;
    memset(&address, 0, sizeof(address));
    address.sun_family = AF_UNIX;
    strncpy(address.sun_path, path, sizeof(address.sun_path) - 1);
    if (bind(server, (struct sockaddr *)&address, sizeof(address)) < 0) {
        fprintf(stderr, "bind %s: %s\n", path, strerror(errno));
        return 1;
    }
    if (listen(server, 4) < 0) {
        perror("listen");
        return 1;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        MXPrepareAX();
        (void)MXFrameworkStatus();
    });
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        while (MXRunning) {
            int client = accept(server, NULL, NULL);
            if (client < 0) {
                if (errno == EINTR) {
                    continue;
                }
                break;
            }
            MXTune(client);
            MXServeClient(client);
        }
        CFRunLoopStop(CFRunLoopGetMain());
    });
    CFRunLoopRun();
    close(server);
    unlink(path);
    return 0;
}
