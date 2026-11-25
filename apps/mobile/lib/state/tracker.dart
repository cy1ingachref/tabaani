import 'dart:async';

import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../data/rider_trace.dart';
import '../data/route.dart';
import '../services/ws.dart';
import '../wire/pb.dart' show VehicleWave;

/// Drives a simulated rider along the TGM timetable for live demos and for the
/// phone-mimic verification. On a real device this is replaced by the platform
/// location stream (see the Kotlin host in android/app/src/main/kotlin).
///
/// The trace is tgm-004's real stop_times profile with a 120 s delay. Crucially,
/// timestamps are seconds AFTER MIDNIGHT — the matcher fits positions against
/// the timetable, so wall-clock epoch seconds would never fit.
class RiderSimulator {
  final IngestClient ingest;
  Timer? _timer;
  int _i = 0;
  static const int _baseSec = 28820; // 08:00:20, first fix of rider_trace
  static const int _batchSize = 20;

  RiderSimulator({required this.ingest});

  bool get running => _timer != null;

  void start() {
    if (_timer != null) return;
    _timer = Timer.periodic(const Duration(milliseconds: 50), (_) => _tick());
    _tick();
  }

  void stop() {
    _timer?.cancel();
    _timer = null;
    _i = 0; // restart from the beginning on the next START
  }

  void _tick() {
    if (_i >= riderTrace.length) {
      stop();
      return;
    }
    final batch = <({double lat, double lon, int tMs})>[];
    for (var n = 0; n < _batchSize && _i < riderTrace.length; n++, _i++) {
      final p = riderTrace[_i];
      batch.add((lat: p[0], lon: p[1], tMs: (_baseSec + _i * 10) * 1000));
    }
    ingest.sendFixBatch(fixes: batch, seqStart: _i - batch.length);
  }

  /// Interpolate a position at fraction t along the drawn route (for a future
  /// real-device location provider; the demo uses the exact trace above).
  (double, double) pointAlongRoute(double t) {
    final segLengths = <double>[];
    var total = 0.0;
    for (var i = 0; i < tgmRoute.length - 1; i++) {
      final a = tgmRoute[i];
      final b = tgmRoute[i + 1];
      final d =
          (b.lat - a.lat) * (b.lat - a.lat) + (b.lon - a.lon) * (b.lon - a.lon);
      segLengths.add(d);
      total += d;
    }
    var target = t * total;
    for (var i = 0; i < segLengths.length; i++) {
      if (target <= segLengths[i] || i == segLengths.length - 1) {
        final f = segLengths[i] == 0
            ? 0.0
            : (target / segLengths[i]).clamp(0.0, 1.0);
        final a = tgmRoute[i];
        final b = tgmRoute[i + 1];
        return (a.lat + (b.lat - a.lat) * f, a.lon + (b.lon - a.lon) * f);
      }
      target -= segLengths[i];
    }
    final last = tgmRoute.last;
    return (last.lat, last.lon);
  }

  void dispose() => stop();
}

/// App-wide state: one ingest socket (rider phone) and one live socket
/// (viewer phone), exposing recent events and the latest vehicles.
class TrackerState extends ChangeNotifier {
  final IngestClient ingest;
  final LiveFeedClient live;
  late final RiderSimulator simulator;

  final List<String> _log = <String>[];
  List<VehicleWave> _vehicles = <VehicleWave>[];
  bool _ingestUp = false;
  bool _liveUp = false;
  StreamSubscription<String>? _ingestSub;
  StreamSubscription<String>? _liveStatusSub;
  StreamSubscription<List<VehicleWave>>? _liveSub;

  TrackerState()
    : ingest = IngestClient(
        sessionToken: 'demo-${DateTime.now().microsecondsSinceEpoch}',
      ),
      live = LiveFeedClient() {
    simulator = RiderSimulator(ingest: ingest);
  }

  List<String> get log => List.unmodifiable(_log);
  List<VehicleWave> get vehicles => _vehicles;
  bool get ingestUp => _ingestUp;
  bool get liveUp => _liveUp;

  Future<void> connect() async {
    _ingestSub = ingest.status.listen(_onLog);
    _liveStatusSub = live.status.listen(_onLog);
    _liveSub = live.vehicles.listen((v) {
      _vehicles = v;
      _add('live frame: ${v.length} vehicle(s)');
      notifyListeners();
    });
    await ingest.connect();
    await live.connect();
    _ingestUp = true;
    _liveUp = true;
    notifyListeners();
  }

  void _onLog(String msg) => _add(msg);

  void _add(String msg) {
    _log.add('${DateTime.now().toIso8601String().substring(11, 19)} $msg');
    if (_log.length > 40) _log.removeAt(0);
    notifyListeners();
  }

  void toggleRider() {
    if (simulator.running) {
      simulator.stop();
      _add('rider stopped');
    } else {
      simulator.start();
      _add('rider started');
    }
    notifyListeners();
  }

  @override
  void dispose() {
    simulator.dispose();
    _ingestSub?.cancel();
    _liveStatusSub?.cancel();
    _liveSub?.cancel();
    ingest.dispose();
    live.dispose();
    super.dispose();
  }
}
