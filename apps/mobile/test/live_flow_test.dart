// Integration test: real backend + the app's own WS clients and protobuf codec.
//
//   flutter test test/live_flow_test.dart --dart-define=API_HOST=127.0.0.1
//
// Requires the backend stack up (`make services-up` from the repo root).
// This is the same wire path the phone UI uses: rider fixes -> /v1/ingest ->
// Redis -> matcher -> /v1/live -> LiveFrame decode. The trace is tgm-004's
// real stop_times profile with a 120 s injected delay, as in the Rust e2e.
import 'dart:async';

import 'package:flutter_test/flutter_test.dart';

import 'package:tabaani/data/rider_trace.dart';
import 'package:tabaani/services/ws.dart';
import 'package:tabaani/wire/pb.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('rider fix appears on the live feed', () async {
    final session = 'flutter-${DateTime.now().microsecondsSinceEpoch}';
    final ingest = IngestClient(sessionToken: session);
    final live = LiveFeedClient();
    await ingest.connect();
    await live.connect();
    await Future<void>.delayed(const Duration(milliseconds: 300));

    final received = Completer<List<VehicleWave>>();
    final sub = live.vehicles.listen((vehicles) {
      if (vehicles.isNotEmpty && !received.isCompleted) {
        received.complete(vehicles);
      }
    });

    // Replay the trace in 20-fix batches with cumulative epoch-second clocks,
    // matching tools/e2e exactly. Subscribed to /v1/live before sending.
    // t_s is seconds AFTER MIDNIGHT (the matcher fits against GTFS timetables),
    // not epoch seconds. 28820 = 08:00:20, the e2e trace's first fix.
    const baseSec = 28820;
    final chunk = <({double lat, double lon, int tMs})>[];
    var seq = 0;
    for (var i = 0; i < riderTrace.length; i++) {
      final p = riderTrace[i];
      chunk.add((lat: p[0], lon: p[1], tMs: (baseSec + i * 10) * 1000));
      if (chunk.length == 20 || i == riderTrace.length - 1) {
        ingest.sendFixBatch(fixes: List.of(chunk), seqStart: seq);
        seq += chunk.length;
        chunk.clear();
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    }

    final vehicles = await received.future.timeout(
      const Duration(seconds: 30),
      onTimeout: () => throw TimeoutException('no live vehicle within 30s'),
    );
    await sub.cancel();
    expect(vehicles, isNotEmpty);
    final v = vehicles.first;
    expect(v.tripId, isNotEmpty);
    expect(v.delayS, isA<double>());
    expect(v.confidence, greaterThanOrEqualTo(1));
    // ignore: avoid_print
    print(
      'LIVE ${v.vehicleId} trip=${v.tripId} '
      'delay=${v.delayS.toStringAsFixed(1)}s conf=${v.confidenceLabel}',
    );

    await ingest.dispose();
    await live.dispose();
  });
}
