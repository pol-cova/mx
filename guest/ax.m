#import "ax.h"

#import <UIKit/UIKit.h>
#include <CoreFoundation/CoreFoundation.h>
#include <dlfcn.h>
#include <mach-o/dyld.h>
#include <objc/message.h>
#include <objc/runtime.h>
#include <stdlib.h>
#include <sys/param.h>
#include <unistd.h>

static pid_t MXLastPid;

static void MXScreenSize(double *width, double *height) {
    *width = 0;
    *height = 0;
    dlopen("/System/Library/Frameworks/UIKit.framework/UIKit", RTLD_NOW);
    Class screenClass = objc_lookUpClass("UIScreen");
    if (!screenClass) {
        return;
    }
    id screen = ((id(*)(id, SEL))objc_msgSend)(screenClass, sel_registerName("mainScreen"));
    if (!screen) {
        return;
    }
    CGRect bounds = ((CGRect(*)(id, SEL))objc_msgSend)(screen, sel_registerName("bounds"));
    *width = bounds.size.width;
    *height = bounds.size.height;
}

typedef void *(*MXCreateSystemWide)(void);
typedef int (*MXGetPid)(void *element, pid_t *pid);
typedef int (*MXPerformAction)(void *element, CFStringRef action);
typedef int (*MXSetAttributeValue)(void *element, CFStringRef name, CFTypeRef value);
typedef CFArrayRef (*MXAttributeNumbers)(NSArray *names);
typedef NSDictionary *(*MXDefaultSnapshotParameters)(void);

static const char *MXAttributeSymbols[] = {
    "XC_kAXXCAttributeElementType",
    "XC_kAXXCAttributeIdentifier",
    "XC_kAXXCAttributeLabel",
    "XC_kAXXCAttributeValue",
    "XC_kAXXCAttributeFrame",
    "XC_kAXXCAttributeChildren",
    NULL};

static NSArray<NSString *> *MXAttributeNames(void) {
    NSMutableArray<NSString *> *names = [NSMutableArray array];
    for (const char **symbol = MXAttributeSymbols; *symbol; symbol++) {
        CFStringRef *ref = (CFStringRef *)dlsym(RTLD_DEFAULT, *symbol);
        if (ref && *ref) {
            [names addObject:(__bridge NSString *)*ref];
        } else {
            [names addObject:[NSString stringWithUTF8String:*symbol]];
        }
    }
    return names;
}

static BOOL MXIsForbiddenSnapshotName(NSString *name) {
    if (name.length == 0) {
        return NO;
    }
    return [name containsString:@"IsVisible"] || [name containsString:@"isVisible"] ||
           [name containsString:@"VisiblePoint"] || [name containsString:@"visiblePoint"] ||
           [name containsString:@"Interactable"] || [name containsString:@"interactable"] ||
           [name containsString:@"Occluded"] || [name containsString:@"occluded"];
}

static NSString *MXRoleName(id type) {
    static NSString *names[] = {
        @"Any",        @"Other",       @"Application", @"Group",     @"Window",
        @"Sheet",      @"Drawer",      @"Alert",       @"Dialog",    @"Button",
        @"RadioButton", @"RadioGroup", @"CheckBox",    @"DisclosureTriangle", @"PopUpButton",
        @"ComboBox",   @"MenuButton",  @"ToolbarButton", @"Popover",   @"Keyboard",
        @"Key",        @"NavigationBar", @"TabBar",    @"TabGroup",  @"Toolbar",
        @"StatusBar", @"Table",        @"TableRow",    @"TableColumn", @"Outline",
        @"OutlineRow", @"Browser",     @"CollectionView", @"Slider",  @"PageIndicator",
        @"ProgressIndicator", @"ActivityIndicator", @"SegmentedControl", @"Picker", @"PickerWheel",
        @"Switch",     @"Toggle",      @"Link",        @"Image",     @"Icon",
        @"SearchField", @"ScrollView", @"ScrollBar",   @"StaticText", @"TextField",
        @"SecureTextField", @"DatePicker", @"TextView", @"Menu",     @"MenuItem",
        @"MenuBar",    @"MenuBarItem"
    };
    NSInteger value = [type respondsToSelector:@selector(integerValue)] ? [type integerValue] : 1;
    if (value >= 0 && (NSUInteger)value < sizeof(names) / sizeof(names[0])) {
        return names[value];
    }
    return @"Unknown";
}

static NSString *MXStringValue(id value) {
    if ([value isKindOfClass:NSString.class] && [value length] > 0) {
        return value;
    }
    if ([value isKindOfClass:NSNumber.class]) {
        return [value stringValue];
    }
    return nil;
}

static unsigned long long MXHashMix(unsigned long long hash, NSString *value) {
    if (value.length == 0) {
        return ((hash << 5) + hash) + 0x1f;
    }
    const char *bytes = value.UTF8String;
    if (!bytes) {
        return ((hash << 5) + hash) + 0x1f;
    }
    while (*bytes) {
        hash = ((hash << 5) + hash) + (unsigned char)*bytes++;
    }
    return ((hash << 5) + hash) + 0x1f;
}

static NSArray<NSNumber *> *MXFrameArray(id frame) {
    if ([frame isKindOfClass:NSDictionary.class]) {
        id x = frame[@"X"] ?: frame[@"x"];
        id y = frame[@"Y"] ?: frame[@"y"];
        id width = frame[@"Width"] ?: frame[@"width"];
        id height = frame[@"Height"] ?: frame[@"height"];
        if (x && y && width && height) {
            return @[ @([x doubleValue]), @([y doubleValue]), @([width doubleValue]), @([height doubleValue]) ];
        }
    }
    if ([frame isKindOfClass:NSValue.class] && strcmp([frame objCType], @encode(CGRect)) == 0) {
        CGRect rect = [frame CGRectValue];
        return @[ @(rect.origin.x), @(rect.origin.y), @(rect.size.width), @(rect.size.height) ];
    }
    return nil;
}

static NSString *MXXctStatus = @"unloaded";

static NSString *MXExecutableDirectory(void) {
    uint32_t size = 0;
    _NSGetExecutablePath(NULL, &size);
    if (size == 0) {
        return nil;
    }
    char *buf = malloc(size);
    if (!buf) {
        return nil;
    }
    if (_NSGetExecutablePath(buf, &size) != 0) {
        free(buf);
        return nil;
    }
    char resolved[PATH_MAX];
    NSString *path;
    if (realpath(buf, resolved)) {
        path = [NSString stringWithUTF8String:resolved];
    } else {
        path = [NSString stringWithUTF8String:buf];
    }
    free(buf);
    return path.stringByDeletingLastPathComponent;
}

static BOOL MXDlopenPath(NSString *path, NSMutableArray<NSString *> *log) {
    if (path.length == 0) {
        return NO;
    }
    if (access(path.fileSystemRepresentation, R_OK) != 0) {
        [log addObject:[NSString stringWithFormat:@"missing %@", path]];
        return NO;
    }
    void *handle = dlopen(path.fileSystemRepresentation, RTLD_NOW | RTLD_GLOBAL);
    if (handle) {
        [log addObject:[NSString stringWithFormat:@"ok %@", path.lastPathComponent]];
        return YES;
    }
    const char *detail = dlerror();
    [log addObject:[NSString stringWithFormat:@"dlopen %@: %s", path, detail ?: "failed"]];
    return NO;
}

static id MXFramework(void) {
    static id framework;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        dlopen("/System/Library/PrivateFrameworks/AXRuntime.framework/AXRuntime", RTLD_NOW | RTLD_GLOBAL);
        NSString *exeDir = MXExecutableDirectory();
        NSMutableArray<NSString *> *log = [NSMutableArray array];
        NSArray<NSString *> *support = exeDir
            ? @[
                  [exeDir stringByAppendingPathComponent:@"Frameworks/XCTestSupport.framework/XCTestSupport"],
                  [exeDir stringByAppendingPathComponent:@"XCTestSupport.framework/XCTestSupport"],
              ]
            : @[];
        NSArray<NSString *> *xct = @[
            exeDir ? [exeDir stringByAppendingPathComponent:@"Frameworks/XCTAutomationSupport.framework/XCTAutomationSupport"] : @"",
            exeDir ? [exeDir stringByAppendingPathComponent:@"XCTAutomationSupport.framework/XCTAutomationSupport"] : @"",
            @"/System/Library/PrivateFrameworks/XCTAutomationSupport.framework/XCTAutomationSupport",
        ];
        BOOL loadedSupport = NO;
        for (NSString *path in support) {
            if (MXDlopenPath(path, log)) {
                loadedSupport = YES;
                break;
            }
        }
        (void)loadedSupport;
        BOOL loadedXct = NO;
        for (NSString *path in xct) {
            if (path.length == 0) {
                continue;
            }
            if (MXDlopenPath(path, log)) {
                loadedXct = YES;
                break;
            }
        }
        Class cls = objc_lookUpClass("XCTAccessibilityFramework");
        const char *initMode = "none";
        if (cls) {
            id allocated = ((id(*)(id, SEL))objc_msgSend)(cls, sel_registerName("alloc"));
            framework = ((id(*)(id, SEL))objc_msgSend)(allocated, sel_registerName("initForRemoteAccess"));
            if (framework) {
                initMode = "remote";
            } else {
                allocated = ((id(*)(id, SEL))objc_msgSend)(cls, sel_registerName("alloc"));
                if ([allocated respondsToSelector:sel_registerName("initForLocalAccess")]) {
                    framework = ((id(*)(id, SEL))objc_msgSend)(allocated, sel_registerName("initForLocalAccess"));
                    if (framework) {
                        initMode = "local";
                    }
                }
            }
        }
        if (framework) {
            MXXctStatus = [NSString stringWithFormat:@"loaded-%s%s", initMode, loadedXct ? "" : "-nosym"];
        } else {
            MXXctStatus = [NSString stringWithFormat:@"unavailable (%@)", [log componentsJoinedByString:@"; "]];
        }
    });
    return framework;
}

NSString *MXFrameworkStatus(void) {
    (void)MXFramework();
    return MXXctStatus ?: @"unloaded";
}

static Class MXElementClass(void) {
    return objc_lookUpClass("XCAccessibilityElement");
}

static pid_t MXFrontmostPid(void) {
    MXGetPid getPid = dlsym(RTLD_DEFAULT, "AXUIElementGetPid");
    MXCreateSystemWide create = dlsym(RTLD_DEFAULT, "AXUIElementCreateSystemWide");
    if (!getPid || !create) {
        return 0;
    }
    void *systemWide = create();
    if (!systemWide) {
        return 0;
    }
    pid_t pid = 0;
    getPid(systemWide, &pid);
    CFRelease(systemWide);
    return pid;
}

static NSDictionary *MXCompact(id node, NSDictionary<NSNumber *, NSString *> *names, NSMutableArray *flat, NSMutableArray *refs) {
    if (![node isKindOfClass:NSDictionary.class]) {
        return nil;
    }
    NSMutableDictionary *element = [NSMutableDictionary dictionary];
    __block id type = nil;
    __block id identifier = nil;
    __block id label = nil;
    __block id value = nil;
    __block id frame = nil;
    __block id children = nil;
    [node enumerateKeysAndObjectsUsingBlock:^(id key, id object, BOOL *stop) {
        NSString *name = [key isKindOfClass:NSNumber.class] ? names[key] : [key description];
        if ([name containsString:@"ElementType"]) {
            type = object;
        } else if ([name containsString:@"Identifier"]) {
            identifier = object;
        } else if ([name containsString:@"Label"]) {
            label = object;
        } else if ([name hasSuffix:@"Value"] && ![name containsString:@"Placeholder"]) {
            value = object;
        } else if ([name containsString:@"Frame"]) {
            frame = object;
        } else if ([name containsString:@"Children"]) {
            children = object;
        }
    }];
    NSString *role = MXRoleName(type);
    NSString *idValue = MXStringValue(identifier);
    NSString *labelValue = MXStringValue(label);
    NSString *valueText = MXStringValue(value);
    if (idValue || labelValue || valueText) {
        if (role) {
            element[@"role"] = role;
        }
        if (idValue) {
            element[@"identifier"] = idValue;
        }
        if (labelValue) {
            element[@"label"] = labelValue;
        }
        if (valueText) {
            element[@"value"] = valueText;
        }
        NSArray *frameArray = MXFrameArray(frame);
        if (frameArray) {
            element[@"frame"] = frameArray;
        }
        [flat addObject:element];
        id raw = node[@"element"] ?: node[@"AXUIElement"];
        [refs addObject:raw ?: [NSNull null]];
    }
    if ([children isKindOfClass:NSArray.class]) {
        for (id child in children) {
            MXCompact(child, names, flat, refs);
        }
    }
    return element;
}

static void MXCollectSnapshot(id snapshot, NSDictionary<NSNumber *, NSString *> *names, NSMutableArray *flat, NSMutableArray *refs) {
    if ([snapshot isKindOfClass:NSArray.class]) {
        for (id child in (NSArray *)snapshot) {
            MXCollectSnapshot(child, names, flat, refs);
        }
        return;
    }
    if (![snapshot isKindOfClass:NSDictionary.class] && [snapshot respondsToSelector:sel_registerName("userTestingSnapshot")]) {
        id inner = ((id(*)(id, SEL))objc_msgSend)(snapshot, sel_registerName("userTestingSnapshot"));
        if (inner && inner != snapshot) {
            MXCollectSnapshot(inner, names, flat, refs);
            return;
        }
    }
    MXCompact(snapshot, names, flat, refs);
}

static NSArray *MXWalk(id element, id framework, NSArray *names, NSMutableArray *flat, NSMutableArray *refs, NSError **error) {
    NSError *readError = nil;
    NSDictionary *read = ((id(*)(id, SEL, id, id, NSError **))objc_msgSend)(
        framework, sel_registerName("attributesForElement:attributes:error:"), element, names, &readError);
    if (!read) {
        if (error) {
            *error = readError;
        }
        return nil;
    }
    NSString *role = MXRoleName(read[@"XC_kAXXCAttributeElementType"] ?: read[@"elementType"]);
    NSString *identifier = MXStringValue(read[@"XC_kAXXCAttributeIdentifier"] ?: read[@"identifier"]);
    NSString *label = MXStringValue(read[@"XC_kAXXCAttributeLabel"] ?: read[@"label"]);
    NSString *value = MXStringValue(read[@"XC_kAXXCAttributeValue"] ?: read[@"value"]);
    if (identifier || label || value) {
        NSMutableDictionary *node = [NSMutableDictionary dictionary];
        node[@"role"] = role ?: @"Unknown";
        if (role) {
            node[@"role"] = role;
        }
        if (identifier) {
            node[@"identifier"] = identifier;
        }
        if (label) {
            node[@"label"] = label;
        }
        if (value) {
            node[@"value"] = value;
        }
        NSArray *frameArray = MXFrameArray(read[@"XC_kAXXCAttributeFrame"] ?: read[@"frame"]);
        if (frameArray) {
            node[@"frame"] = frameArray;
        }
        [flat addObject:node];
        [refs addObject:element];
    }
    id children = read[@"XC_kAXXCAttributeChildren"] ?: read[@"children"];
    if ([children isKindOfClass:NSArray.class]) {
        for (id child in children) {
            MXWalk(child, framework, names, flat, refs, error);
        }
    }
    return flat;
}

static NSMutableArray *MXLastRefs(void) {
    static NSMutableArray *refs;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        refs = [NSMutableArray array];
    });
    return refs;
}

static NSMutableArray *MXLastElements(void) {
    static NSMutableArray *elements;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        elements = [NSMutableArray array];
    });
    return elements;
}

typedef void *(*MXCreateApp)(pid_t pid);
typedef int (*MXCopyAttr)(void *element, CFStringRef name, CFTypeRef *value);
typedef Boolean (*MXBoolFn)(void);
typedef int (*MXLastErrorFn)(void);

static void *MXAXHandle;
static MXCopyAttr MXCopyFn;

static CFStringRef MXC(const char *name) {
    if (!MXAXHandle || !name) {
        return NULL;
    }
    CFStringRef *symbol = (CFStringRef *)dlsym(MXAXHandle, name);
    return symbol ? *symbol : NULL;
}

static CFTypeRef MXCopyRaw(void *element, CFStringRef name) {
    if (!element || !name || !MXCopyFn) {
        return NULL;
    }
    CFTypeRef value = NULL;
    MXCopyFn(element, name, &value);
    return value;
}

static CFTypeRef MXCopyAny(void *element, const char *xcName, CFStringRef classic) {
    CFStringRef xc = MXC(xcName);
    CFTypeRef value = MXCopyRaw(element, xc);
    if (value) {
        return value;
    }
    return MXCopyRaw(element, classic);
}

static NSString *MXCFString(CFTypeRef value) {
    if (!value) {
        return nil;
    }
    if (CFGetTypeID(value) == CFStringGetTypeID()) {
        return (__bridge NSString *)value;
    }
    if (CFGetTypeID(value) == CFNumberGetTypeID()) {
        return [(__bridge NSNumber *)value stringValue];
    }
    return [(__bridge id)value description];
}

static void MXWalkAX(void *element, NSMutableArray *flat, NSMutableArray *refs) {
    if (!element) {
        return;
    }
    CFTypeRef identifier = MXCopyAny(element, "kAXXCAttributeIdentifier", CFSTR("AXIdentifier"));
    CFTypeRef label = MXCopyAny(element, "kAXXCAttributeLabel", CFSTR("AXLabel"));
    if (!label) {
        label = MXCopyRaw(element, CFSTR("AXTitle"));
    }
    CFTypeRef value = MXCopyAny(element, "kAXXCAttributeValue", CFSTR("AXValue"));
    CFTypeRef role = MXCopyAny(element, "kAXXCAttributeElementType", CFSTR("AXRole"));
    CFTypeRef frame = MXCopyAny(element, "kAXXCAttributeFrame", CFSTR("AXFrame"));
    NSString *idValue = MXStringValue(MXCFString(identifier));
    NSString *labelValue = MXStringValue(MXCFString(label));
    NSString *valueText = MXStringValue(MXCFString(value));
    if (idValue || labelValue || valueText) {
        NSMutableDictionary *node = [NSMutableDictionary dictionary];
        node[@"role"] = MXRoleName((__bridge id)role);
        if (idValue) {
            node[@"identifier"] = idValue;
        }
        if (labelValue) {
            node[@"label"] = labelValue;
        }
        if (valueText) {
            node[@"value"] = valueText;
        }
        NSArray *frameArray = MXFrameArray((__bridge id)frame);
        if (frameArray) {
            node[@"frame"] = frameArray;
        }
        [flat addObject:node];
        CFRetain(element);
        [refs addObject:(__bridge_transfer id)element];
    }
    CFTypeRef children = MXCopyAny(element, "kAXXCAttributeChildren", CFSTR("AXChildren"));
    if (children && CFGetTypeID(children) == CFArrayGetTypeID()) {
        CFArrayRef array = (CFArrayRef)children;
        CFIndex count = CFArrayGetCount(array);
        for (CFIndex i = 0; i < count; i++) {
            MXWalkAX((void *)CFArrayGetValueAtIndex(array, i), flat, refs);
        }
    }
    if (identifier) {
        CFRelease(identifier);
    }
    if (label) {
        CFRelease(label);
    }
    if (value) {
        CFRelease(value);
    }
    if (role) {
        CFRelease(role);
    }
    if (frame) {
        CFRelease(frame);
    }
    if (children) {
        CFRelease(children);
    }
}

static NSString *MXCopyStatus(void *element, CFStringRef name) {
    if (!name) {
        return @"key=null";
    }
    CFTypeRef value = NULL;
    int status = MXCopyFn ? MXCopyFn(element, name, &value) : -1;
    NSString *desc = [NSString stringWithFormat:@"st=%d t=%@", status, value ? [(__bridge id)value class] : nil];
    if (value) {
        CFRelease(value);
    }
    return desc;
}

void MXPrepareAX(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        CFPreferencesSetAppValue(CFSTR("AccessibilityEnabled"), kCFBooleanTrue, CFSTR("com.apple.Accessibility"));
        CFPreferencesSetAppValue(CFSTR("ApplicationAccessibilityEnabled"), kCFBooleanTrue, CFSTR("com.apple.Accessibility"));
        CFPreferencesAppSynchronize(CFSTR("com.apple.Accessibility"));
        MXAXHandle = dlopen("/System/Library/PrivateFrameworks/AXRuntime.framework/AXRuntime", RTLD_NOW | RTLD_GLOBAL);
        MXCopyFn = dlsym(RTLD_DEFAULT, "AXUIElementCopyAttributeValue");
        typedef void (*MXSetClient)(int);
        MXSetClient setClient = dlsym(MXAXHandle, "_AXSetRequestingClient");
        if (setClient) {
            setClient(8);
        }
        typedef void (*MXOverride)(int);
        MXOverride overrideClient = dlsym(MXAXHandle, "AXOverrideRequestingClientType");
        if (overrideClient) {
            overrideClient(8);
        }
    });
}

static NSDictionary *MXSnapshotAX(pid_t pid, NSError **error) {
    MXPrepareAX();
    if (!MXAXHandle) {
        if (error) {
            const char *detail = dlerror() ?: "dlopen failed";
            *error = [NSError errorWithDomain:@"mx.guest" code:1 userInfo:@{
                NSLocalizedDescriptionKey: [NSString stringWithFormat:@"AXRuntime dlopen failed: %s", detail]
            }];
        }
        return nil;
    }
    MXCreateApp create = dlsym(MXAXHandle, "_AXUIElementCreateAppElementWithPid");
    if (!create) {
        create = dlsym(MXAXHandle, "AXUIElementCreateAppElementWithPid");
    }
    typedef void *(*MXCreateWide)(void);
    MXCreateWide createWide = dlsym(MXAXHandle, "AXUIElementCreateSystemWide");
    MXCreateWide sharedWide = dlsym(MXAXHandle, "AXUIElementSharedSystemWide");
    MXCreateWide sharedApp = dlsym(MXAXHandle, "AXUIElementSharedSystemApp");
    if (!create && !createWide && !sharedWide) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:1 userInfo:@{NSLocalizedDescriptionKey: @"AXUIElement create missing"}];
        }
        return nil;
    }
    if (pid <= 0) {
        pid = MXFrontmostPid();
    }
    void *app = NULL;
    const char *mode = "none";
    if (create && pid > 0) {
        app = create(pid);
        mode = "createPid";
    }
    if (!app && sharedApp) {
        app = sharedApp();
        mode = "sharedApp";
    }
    if (!app && sharedWide) {
        app = sharedWide();
        mode = "sharedWide";
    }
    if (!app && createWide) {
        app = createWide();
        mode = "systemWide";
    }
    if (!app) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:2 userInfo:@{NSLocalizedDescriptionKey: @"Could not resolve the frontmost application"}];
        }
        return nil;
    }
    MXGetPid getPid = dlsym(MXAXHandle, "AXUIElementGetPid");
    if (getPid) {
        pid_t resolved = 0;
        getPid(app, &resolved);
        if (resolved > 0) {
            pid = resolved;
        }
    }
    NSMutableArray *flat = [NSMutableArray array];
    NSMutableArray *refs = [NSMutableArray array];
    CFTypeRef running = MXCopyAny(app, "kAXXCAttributeRunningApplications", CFSTR("AXRunningApplications"));
    if (running && CFGetTypeID(running) == CFArrayGetTypeID()) {
        CFArrayRef array = (CFArrayRef)running;
        CFIndex count = CFArrayGetCount(array);
        for (CFIndex i = 0; i < count; i++) {
            void *candidate = (void *)CFArrayGetValueAtIndex(array, i);
            if (pid > 0 && getPid) {
                pid_t candidatePid = 0;
                getPid(candidate, &candidatePid);
                if (candidatePid > 0 && candidatePid != pid) {
                    continue;
                }
            }
            MXWalkAX(candidate, flat, refs);
        }
        CFRelease(running);
    } else {
        if (running) {
            CFRelease(running);
        }
        MXWalkAX(app, flat, refs);
    }
    if (flat.count == 0) {
        Class elementClass = objc_lookUpClass("AXUIElement");
        id systemWide = nil;
        if (elementClass && [elementClass respondsToSelector:sel_registerName("systemWideElement")]) {
            systemWide = ((id(*)(id, SEL))objc_msgSend)(elementClass, sel_registerName("systemWideElement"));
        }
        if (systemWide && [systemWide respondsToSelector:sel_registerName("attributeValueForKey:")]) {
            id objcChildren = ((id(*)(id, SEL, id))objc_msgSend)(systemWide, sel_registerName("attributeValueForKey:"), @"AXChildren");
            if ([objcChildren isKindOfClass:NSArray.class]) {
                for (id child in objcChildren) {
                    void *raw = (__bridge void *)child;
                    if ([child respondsToSelector:sel_registerName("axElement")]) {
                        raw = ((void *(*)(id, SEL))objc_msgSend)(child, sel_registerName("axElement"));
                    }
                    MXWalkAX(raw, flat, refs);
                }
            }
        }
    }
    if (flat.count == 0) {
        MXBoolFn apiEnabled = dlsym(MXAXHandle, "AXAPIEnabled");
        MXBoolFn canContact = dlsym(MXAXHandle, "AXProcessCanContactSystemWideServer");
        MXBoolFn deserve = dlsym(MXAXHandle, "AXDoesRequestingClientDeserveAutomation");
        MXLastErrorFn lastError = dlsym(MXAXHandle, "AXUIElementLastGlobalError");
        NSMutableArray *found = [NSMutableArray array];
        [found addObject:[NSString stringWithFormat:@"mode=%s", mode]];
        [found addObject:[NSString stringWithFormat:@"pid=%d", pid]];
        [found addObject:[NSString stringWithFormat:@"api=%d", apiEnabled ? apiEnabled() : -1]];
        [found addObject:[NSString stringWithFormat:@"contact=%d", canContact ? canContact() : -1]];
        [found addObject:[NSString stringWithFormat:@"deserve=%d", deserve ? deserve() : -1]];
        [found addObject:[NSString stringWithFormat:@"last=%d", lastError ? lastError() : -1]];
        [found addObject:[NSString stringWithFormat:@"AXChildren:%@", MXCopyStatus(app, CFSTR("AXChildren"))]];
        [found addObject:[NSString stringWithFormat:@"AXLabel:%@", MXCopyStatus(app, CFSTR("AXLabel"))]];
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:9 userInfo:@{
                NSLocalizedDescriptionKey: [NSString stringWithFormat:@"empty AX tree (%@)", [found componentsJoinedByString:@","]]
            }];
        }
        return nil;
    }
    [MXLastElements() removeAllObjects];
    [MXLastElements() addObjectsFromArray:flat];
    [MXLastRefs() removeAllObjects];
    [MXLastRefs() addObjectsFromArray:refs];
    unsigned long long hash = 5381;
    for (NSDictionary *element in flat) {
        hash = MXHashMix(hash, element[@"role"]);
        hash = MXHashMix(hash, element[@"identifier"]);
        hash = MXHashMix(hash, element[@"label"]);
        hash = MXHashMix(hash, element[@"value"]);
    }
    double width = 0;
    double height = 0;
    MXScreenSize(&width, &height);
    return @{
        @"pid": @(pid),
        @"width": @(width > 0 ? width : 390),
        @"height": @(height > 0 ? height : 844),
        @"hash": [NSString stringWithFormat:@"%016llx", hash],
        @"truncated": @NO,
        @"elements": flat
    };
}

NSDictionary *MXSnapshotTree(pid_t pid, NSError **error) {
    id framework = MXFramework();
    Class elementClass = MXElementClass();
    if (!framework || !elementClass) {
        return MXSnapshotAX(pid, error);
    }
    if (pid <= 0) {
        pid = MXFrontmostPid();
    }
    MXLastPid = pid;
    id app = ((id(*)(id, SEL, pid_t))objc_msgSend)(elementClass, sel_registerName("elementWithProcessIdentifier:"), pid);
    if (!app) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:2 userInfo:@{NSLocalizedDescriptionKey: @"Could not resolve the frontmost application"}];
        }
        return nil;
    }
    if ([framework respondsToSelector:sel_registerName("setAXRequestingClientWithElement:")]) {
        ((void (*)(id, SEL, id))objc_msgSend)(framework, sel_registerName("setAXRequestingClientWithElement:"), app);
    }
    void *rawApp = NULL;
    if ([app respondsToSelector:sel_registerName("AXUIElement")]) {
        rawApp = ((void *(*)(id, SEL))objc_msgSend)(app, sel_registerName("AXUIElement"));
    }
    typedef int (*MXSetTimeout)(void *element, float timeout);
    MXSetTimeout setTimeout = dlsym(RTLD_DEFAULT, "AXUIElementSetMessagingTimeout");
    typedef void (*MXSetGlobalTimeout)(float timeout);
    MXSetGlobalTimeout setGlobalTimeout = dlsym(RTLD_DEFAULT, "_AXUIElementSetGlobalTimeout");
    if (setGlobalTimeout) {
        setGlobalTimeout(3.0f);
    }
    if (setTimeout && rawApp) {
        setTimeout(rawApp, 3.0f);
    }
    NSArray *names = [MXAttributeNames() copy];
    MXAttributeNumbers numbersFn = dlsym(RTLD_DEFAULT, "XCAXAccessibilityAttributesForStringAttributes");
    MXDefaultSnapshotParameters defaultsFn = dlsym(RTLD_DEFAULT, "XCTDefaultSnapshotParameters");
    NSMutableArray *flat = [NSMutableArray array];
    NSMutableArray *refs = [NSMutableArray array];
    BOOL snapshotted = NO;
    NSError *snapError = nil;
    SEL snapshotSel = sel_registerName("userTestingSnapshotForElement:options:error:");
    if (numbersFn && [framework respondsToSelector:snapshotSel]) {
        NSArray *numbers = (__bridge NSArray *)numbersFn(names);
        NSMutableDictionary *inverse = [NSMutableDictionary dictionary];
        NSMutableArray *allowed = [NSMutableArray array];
        [numbers enumerateObjectsUsingBlock:^(id number, NSUInteger index, BOOL *stop) {
            (void)stop;
            NSString *name = index < names.count ? names[index] : [number description];
            if (MXIsForbiddenSnapshotName(name)) {
                return;
            }
            [allowed addObject:number];
            if ([number isKindOfClass:NSNumber.class]) {
                inverse[number] = name;
            }
        }];
        NSMutableDictionary *options = defaultsFn ? [defaultsFn() mutableCopy] : [NSMutableDictionary dictionary];
        if (!options) {
            options = [NSMutableDictionary dictionary];
        }
        options[@"attributes"] = allowed;
        options[@"timeout"] = @3.0;
        options[@"queryExecutionTimeout"] = @3.0;
        id snapshot = ((id(*)(id, SEL, id, id, NSError **))objc_msgSend)(
            framework, snapshotSel, app, options, &snapError);
        if (!snapshot && rawApp) {
            snapError = nil;
            snapshot = ((id(*)(id, SEL, id, id, NSError **))objc_msgSend)(
                framework, snapshotSel, (__bridge id)rawApp, options, &snapError);
        }
        if (snapshot) {
            MXCollectSnapshot(snapshot, inverse, flat, refs);
            snapshotted = YES;
        }
    }
    if (!snapshotted || flat.count == 0) {
        NSError *walkError = nil;
        MXWalk(app, framework, names, flat, refs, &walkError);
        if (!snapError) {
            snapError = walkError;
        }
    }
    if (flat.count == 0) {
        NSError *axError = nil;
        NSDictionary *axTree = MXSnapshotAX(pid, &axError);
        if (axTree && [axTree[@"elements"] isKindOfClass:NSArray.class] && [axTree[@"elements"] count] > 0) {
            return axTree;
        }
        if (error) {
            NSString *detail = snapError.localizedDescription ?: @"no XCT snapshot";
            NSString *axDetail = axError.localizedDescription ?: @"no AX tree";
            *error = [NSError errorWithDomain:@"mx.guest" code:9 userInfo:@{
                NSLocalizedDescriptionKey: [NSString stringWithFormat:@"empty XCT tree (%@; %@); %@", MXFrameworkStatus(), detail, axDetail]
            }];
        }
        return nil;
    }
    [MXLastElements() removeAllObjects];
    [MXLastElements() addObjectsFromArray:flat];
    [MXLastRefs() removeAllObjects];
    [MXLastRefs() addObjectsFromArray:refs];
    unsigned long long hash = 5381;
    for (NSDictionary *element in flat) {
        hash = MXHashMix(hash, element[@"role"]);
        hash = MXHashMix(hash, element[@"identifier"]);
        hash = MXHashMix(hash, element[@"label"]);
        hash = MXHashMix(hash, element[@"value"]);
    }
    double width = 0;
    double height = 0;
    MXScreenSize(&width, &height);
    return @{
        @"pid": @(pid),
        @"width": @(width > 0 ? width : 390),
        @"height": @(height > 0 ? height : 844),
        @"hash": [NSString stringWithFormat:@"%016llx", hash],
        @"truncated": @NO,
        @"elements": flat
    };
}

NSDictionary *MXSnapshot(pid_t pid, NSString *ifHashNot, BOOL wantTree, NSError **error) {
    NSDictionary *tree = MXSnapshotTree(pid, error);
    if (!tree) {
        return nil;
    }
    NSString *hash = [tree[@"hash"] isKindOfClass:NSString.class] ? tree[@"hash"] : nil;
    if (ifHashNot.length > 0 && hash && [hash isEqualToString:ifHashNot]) {
        NSMutableDictionary *reply = [tree mutableCopy];
        [reply removeObjectForKey:@"elements"];
        reply[@"unchanged"] = @YES;
        return reply;
    }
    if (!wantTree) {
        NSMutableDictionary *reply = [tree mutableCopy];
        [reply removeObjectForKey:@"elements"];
        return reply;
    }
    return tree;
}

static id MXMatch(NSString *identifier, NSString *label, NSString *role) {
    NSArray *elements = MXLastElements();
    NSArray *refs = MXLastRefs();
    id found = nil;
    NSUInteger matches = 0;
    for (NSUInteger i = 0; i < elements.count; i++) {
        NSDictionary *element = elements[i];
        if (identifier && ![element[@"identifier"] isEqual:identifier]) {
            continue;
        }
        if (label && ![element[@"label"] isEqual:label]) {
            continue;
        }
        if (role && ![element[@"role"] isEqual:role]) {
            continue;
        }
        found = refs[i];
        matches += 1;
    }
    return matches == 1 ? found : nil;
}

BOOL MXPress(pid_t pid, NSString *identifier, NSString *label, NSString *role, NSError **error) {
    if (pid > 0) {
        MXLastPid = pid;
    }
    if (MXLastElements().count == 0) {
        NSError *snapError = nil;
        (void)MXSnapshotTree(MXLastPid, &snapError);
    }
    id target = MXMatch(identifier, label, role);
    if (!target || [target isEqual:[NSNull null]]) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:3 userInfo:@{NSLocalizedDescriptionKey: @"Press matched zero or several elements"}];
        }
        return NO;
    }
    id framework = MXFramework();
    if (framework && [framework respondsToSelector:sel_registerName("performAction:onElement:value:error:")]) {
        NSError *pressError = nil;
        BOOL ok = ((BOOL(*)(id, SEL, id, id, id, NSError **))objc_msgSend)(
            framework,
            sel_registerName("performAction:onElement:value:error:"),
            @"AXPress",
            target,
            nil,
            &pressError);
        if (ok) {
            return YES;
        }
    }
    void *raw = (__bridge void *)target;
    if ([target respondsToSelector:sel_registerName("AXUIElement")]) {
        raw = ((void *(*)(id, SEL))objc_msgSend)(target, sel_registerName("AXUIElement"));
    }
    MXPerformAction perform = dlsym(RTLD_DEFAULT, "AXUIElementPerformAction");
    if (!perform || !raw) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:4 userInfo:@{NSLocalizedDescriptionKey: @"AXUIElementPerformAction is unavailable"}];
        }
        return NO;
    }
    int status = perform(raw, CFSTR("AXPress"));
    if (status != 0) {
        status = perform(raw, CFSTR("FKAPress"));
    }
    if (status != 0) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:5 userInfo:@{NSLocalizedDescriptionKey: @"AX press failed"}];
        }
        return NO;
    }
    return YES;
}

BOOL MXSetValue(NSString *text, NSError **error) {
    if (MXLastElements().count == 0) {
        (void)MXSnapshotTree(MXLastPid, error);
    }
    id target = nil;
    NSArray *elements = MXLastElements();
    NSArray *refs = MXLastRefs();
    for (NSUInteger i = 0; i < elements.count; i++) {
        NSDictionary *element = elements[i];
        if ([element[@"role"] isEqual:@"TextField"] || [element[@"role"] isEqual:@"SearchField"] || [element[@"role"] isEqual:@"TextView"] || [element[@"role"] isEqual:@"SecureTextField"]) {
            target = refs[i];
            break;
        }
    }
    if (!target || [target isEqual:[NSNull null]]) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:6 userInfo:@{NSLocalizedDescriptionKey: @"No text field is focused"}];
        }
        return NO;
    }
    void *raw = (__bridge void *)target;
    if ([target respondsToSelector:sel_registerName("AXUIElement")]) {
        raw = ((void *(*)(id, SEL))objc_msgSend)(target, sel_registerName("AXUIElement"));
    }
    MXSetAttributeValue setValue = dlsym(RTLD_DEFAULT, "AXUIElementSetAttributeValue");
    if (!setValue || !raw) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:7 userInfo:@{NSLocalizedDescriptionKey: @"AXUIElementSetAttributeValue is unavailable"}];
        }
        return NO;
    }
    int status = setValue(raw, CFSTR("AXValue"), (__bridge CFTypeRef)text);
    if (status != 0) {
        if (error) {
            *error = [NSError errorWithDomain:@"mx.guest" code:8 userInfo:@{NSLocalizedDescriptionKey: @"Setting AXValue failed"}];
        }
        return NO;
    }
    return YES;
}
