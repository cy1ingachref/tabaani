# Tabaani Mobile

Flutter client for the Tabaani live transit tracker. The Android host is written
in Kotlin (`android/app/src/main/kotlin/tn/tabaani/tabaani/MainActivity.kt`).

## What it does

- **Map** — MapLibre with the dark `https://styles.maptoolkit.org/dark.json`
  base style, the TGM corridor polyline, stop markers, and live vehicle markers.
- **START RIDER** — replays tgm-004's real stop_times trace (120 s delay) through
  the app's own protobuf encoder to `/v1/ingest` over a binary WebSocket.
- **Live** — vehicles decoded from `/v1/live` binary protobuf frames, with
  trip id, fitted delay, and confidence (Scheduled / Estimated / LIVE).
- **Log** — the actual wire events, useful for demos and debugging.

The Dart protobuf codec in `lib/wire/pb.dart` mirrors the Rust contract in
`services/api/src/pb.rs` byte-for-byte (field numbers, zigzag sint32, fixed32).

## Run against the local backend

From the repo root:

```bash
make stack-up        # postgres + redis + api + matcher, GTFS loaded
cd apps/mobile
flutter pub get
flutter run -d chrome --dart-define=API_HOST=127.0.0.1 --dart-define=API_PORT=8080
```

For a phone/emulator, use `--dart-define=API_HOST=<host-ip>` (a device cannot
reach `127.0.0.1` of the development machine).

## Verify

```bash
flutter analyze
flutter test --exclude-tags integration --dart-define=API_HOST=127.0.0.1
flutter build apk --debug
```

`test/live_flow_test.dart` is a real integration check: it starts a rider,
subscribes to the live feed, and asserts a identified vehicle appears with the
injected 120 s delay. It requires the backend stack to be running. Last run:

```
LIVE tgm:tgm-004 trip=tgm-004 delay=120.0s conf=Estimated
```

The debug APK builds to `build/app/outputs/flutter-apk/app-debug.apk`
(`tn.tabaani.tabaani`, minSdk 24, targetSdk 36).

## Note on timestamps

The backend fits positions against the GTFS timetable, so fix timestamps are
**seconds after midnight**, not epoch seconds. `RiderSimulator` and
`sendFixBatch` use this convention; using wall-clock epoch seconds produces
"no scheduled trip fits the observed track" and no live vehicle.
