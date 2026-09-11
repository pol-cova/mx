SDK ?= iphonesimulator
MIN_OS ?= 16.0
ARCH ?= arm64
CLANG ?= xcrun -sdk $(SDK) clang
SYSROOT := $(shell xcrun --sdk $(SDK) --show-sdk-path)
PLATFORM := $(shell xcrun --sdk $(SDK) --show-sdk-platform-path)
PLATFORM_PF := $(PLATFORM)/Developer/Library/PrivateFrameworks
CFLAGS := -fobjc-arc -fobjc-weak -Wall -Werror -isysroot $(SYSROOT) -arch $(ARCH) -mios-simulator-version-min=$(MIN_OS)
LIBS := -framework Foundation -framework UIKit
RPATH := -Wl,-rpath,@executable_path/Frameworks -Wl,-rpath,@executable_path
SOURCES := mx-guest.m server.m ax.m
OUT ?= mx-guest
FRAMEWORKS_DIR := $(dir $(OUT))Frameworks
APP ?= $(dir $(OUT))MxGuest.app

.PHONY: all
all: $(OUT)

$(OUT): $(SOURCES) protocol.h ax.h mx-guest.entitlements
	$(CLANG) $(CFLAGS) $(SOURCES) $(LIBS) $(RPATH) -o $@
	mkdir -p "$(FRAMEWORKS_DIR)"
	rm -rf "$(FRAMEWORKS_DIR)/XCTAutomationSupport.framework" "$(FRAMEWORKS_DIR)/XCTestSupport.framework"
	ditto "$(PLATFORM_PF)/XCTAutomationSupport.framework" "$(FRAMEWORKS_DIR)/XCTAutomationSupport.framework"
	ditto "$(PLATFORM_PF)/XCTestSupport.framework" "$(FRAMEWORKS_DIR)/XCTestSupport.framework"
	codesign --force --sign - $@

# 0B: real app identity (not spawn-with-embedded APPL plist).
#   make app OUT=/path/mx-guest
#   xcrun simctl install UDID $(dir OUT)MxGuest.app
#   xcrun simctl launch --terminate-running-process UDID dev.mx.guest serve --socket /tmp/mx-UDID.sock
.PHONY: app
app: $(OUT)
	rm -rf "$(APP)"
	mkdir -p "$(APP)/Frameworks"
	cp "$(OUT)" "$(APP)/mx-guest"
	printf '%s\n' \
	  '<?xml version="1.0" encoding="UTF-8"?>' \
	  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
	  '<plist version="1.0"><dict>' \
	  '<key>CFBundleIdentifier</key><string>dev.mx.guest</string>' \
	  '<key>CFBundleName</key><string>MxGuest</string>' \
	  '<key>CFBundleDisplayName</key><string>MxGuest</string>' \
	  '<key>CFBundleExecutable</key><string>mx-guest</string>' \
	  '<key>CFBundleVersion</key><string>1</string>' \
	  '<key>CFBundleShortVersionString</key><string>1.0</string>' \
	  '<key>CFBundlePackageType</key><string>APPL</string>' \
	  '<key>MinimumOSVersion</key><string>$(MIN_OS)</string>' \
	  '<key>LSRequiresIPhoneOS</key><true/>' \
	  '<key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>' \
	  '<key>CFBundleSupportedPlatforms</key><array><string>iPhoneSimulator</string></array>' \
	  '</dict></plist>' > "$(APP)/Info.plist"
	printf 'APPL????' > "$(APP)/PkgInfo"
	ditto "$(FRAMEWORKS_DIR)/XCTAutomationSupport.framework" "$(APP)/Frameworks/XCTAutomationSupport.framework"
	ditto "$(FRAMEWORKS_DIR)/XCTestSupport.framework" "$(APP)/Frameworks/XCTestSupport.framework"
	codesign --force --sign - --entitlements mx-guest.entitlements "$(APP)/mx-guest"
	codesign --force --sign - --entitlements mx-guest.entitlements "$(APP)"

.PHONY: clean
clean:
	rm -f $(OUT) mx-guest
	rm -rf "$(FRAMEWORKS_DIR)/XCTAutomationSupport.framework" "$(FRAMEWORKS_DIR)/XCTestSupport.framework" "$(APP)"
