# Two-minute demo

Mx controls an iOS app through accessibility labels and identifiers. This demo shows a warm launch, two UI changes, a screenshot, and one memory snapshot.

## Prepare

Install Mx and AXe before the presentation. Keep build output on a volume with at least 2 GiB free.

```sh
CARGO_TARGET_DIR=/path/with/free/space sh scripts/install.sh
mx doctor
mx devices
```

Choose one simulator UDID and build MxDemo once:

```sh
DEVICE=SIMULATOR_UDID

mx run \
  --project examples/MxDemo \
  --scheme MxDemo \
  --device "$DEVICE" \
  --inspect-ui

mx stop --device "$DEVICE" --bundle-id dev.mx.demo
```

Before presenting, check that `mx doctor` returns `"ready": true`, the simulator is booted, MxDemo is installed, and the counter starts at zero. The demo does not need network access.

## Run

Start with: "Mx gives an agent a control layer for the iOS Simulator, so it can inspect and operate an app without screen coordinates."

```sh
mx relaunch --device "$DEVICE" --inspect-ui

mx tap --device "$DEVICE" --id increment
mx ui --device "$DEVICE"

mx tap --device "$DEVICE" --id name
mx type --device "$DEVICE" "Café ☕️"
mx tap --device "$DEVICE" --id greet
mx ui --device "$DEVICE"

mx screenshot --device "$DEVICE"
mx metrics --device "$DEVICE"
```

The output should contain `Count: 0`, `Count: 1`, and `Hello, Café ☕️!`. The screenshot shows the final screen. The metrics response reports the footprint of the simulator process tree.

## Performance numbers

The demo uses a warm relaunch. The published median is 235 ms. A three-state capture flow has a 2.63 second median and 3.57 second p95 over 20 runs.

The slim profile measured 832 MiB idle on one simulator. Two simulators measured 1,840 MiB combined at idle and 2,245 MiB with both demo apps running. These numbers came from one 16 GiB Mac with Xcode 26.5 and iOS 26.5.

Do not apply the slim profile during the presentation. It reboots the simulator and adds about 40 seconds. Prepare it beforehand if memory reduction is part of the demo.
