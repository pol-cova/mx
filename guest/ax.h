#ifndef MX_AX_H
#define MX_AX_H

#import <Foundation/Foundation.h>
#include <sys/types.h>

void MXPrepareAX(void);
NSString *_Nonnull MXFrameworkStatus(void);
NSDictionary *_Nullable MXSnapshotTree(pid_t pid, NSError *_Nullable *_Nullable error);
NSDictionary *_Nullable MXSnapshot(pid_t pid, NSString *_Nullable ifHashNot, BOOL wantTree, NSError *_Nullable *_Nullable error);
BOOL MXPress(pid_t pid, NSString *_Nullable identifier, NSString *_Nullable label, NSString *_Nullable role, NSError *_Nullable *_Nullable error);
BOOL MXSetValue(NSString *_Nonnull text, NSError *_Nullable *_Nullable error);

#endif
