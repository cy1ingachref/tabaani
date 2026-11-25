import 'dart:async';

import 'package:web_socket_channel/web_socket_channel.dart';

import '../wire/pb.dart';

/// Configuration for the local backend. `--dart-define=API_HOST=...` overrides
/// the host so the same build works on a device, emulator, or desktop browser.
class ApiConfig {
  static const host = String.fromEnvironment(
    'API_HOST',
    defaultValue: '127.0.0.1',
  );
  static const port = String.fromEnvironment('API_PORT', defaultValue: '8080');

  static String get wsBase => 'ws://$host:$port';
  static String get httpBase => 'http://$host:$port';
}

/// Sends delta-encoded position fixes to `/v1/ingest` as binary protobuf.
class IngestClient {
  final String sessionToken;
  final String datasetVersion;
  WebSocketChannel? _channel;
  int _seq = 0;
  int _lastLatE7 = 0;
  int _lastLonE7 = 0;
  int _lastTsMs = 0;
  final StreamController<String> _status = StreamController.broadcast();

  IngestClient({required this.sessionToken, this.datasetVersion = '0.1.0-dev'});

  Stream<String> get status => _status.stream;
  bool get connected => _channel != null;

  Future<void> connect() async {
    _channel = WebSocketChannel.connect(
      Uri.parse('${ApiConfig.wsBase}/v1/ingest'),
    );
    _status.add('connected');
    // Consume the server's JSON acks so the socket does not back-pressure.
    _channel!.stream.listen(
      (msg) => _status.add('ack: $msg'),
      onError: (e) => _status.add('error: $e'),
      onDone: () => _status.add('disconnected'),
    );
  }

  /// Send one absolute fix. The wire carries deltas from the previous fix,
  /// exactly as the Rust decoder expects.
  void sendFix({
    required double lat,
    required double lon,
    required DateTime ts,
    int accuracyDm = 50,
    int speedCms = 0,
    int bearingDeg = 0,
  }) {
    final channel = _channel;
    if (channel == null) return;
    final latE7 = (lat * 1e7).round();
    final lonE7 = (lon * 1e7).round();
    final tsMs = ts.millisecondsSinceEpoch;
    final batch = PingBatchWave(
      sessionToken: sessionToken,
      seqStart: _seq,
      datasetVersion: datasetVersion,
      fixes: [
        FixWave(
          dLatE7: latE7 - _lastLatE7,
          dLonE7: lonE7 - _lastLonE7,
          dTsMs: _lastTsMs == 0 ? 0 : tsMs - _lastTsMs,
          accuracyDm: accuracyDm,
          speedCms: speedCms,
          bearingDeg: bearingDeg,
        ),
      ],
    );
    channel.sink.add(batch.encode());
    _seq += 1;
    _lastLatE7 = latE7;
    _lastLonE7 = lonE7;
    _lastTsMs = tsMs;
    _status.add('sent fix #$_seq');
  }

  /// Send one batch of absolute fixes with cumulative timestamps.
  ///
  /// The backend's `absolute_fixes` SUMS d_ts_ms across fixes and sessions, so
  /// deltas must be relative to a shared origin within a rider's session —
  /// exactly like `tools/e2e` chunks its rider trace.
  void sendFixBatch({
    required List<({double lat, double lon, int tMs})> fixes,
    required int seqStart,
  }) {
    final channel = _channel;
    if (channel == null || fixes.isEmpty) return;
    final proto = <FixWave>[];
    var prevLat = 0;
    var prevLon = 0;
    var prevTs = 0;
    for (final f in fixes) {
      final latE7 = (f.lat * 1e7).round();
      final lonE7 = (f.lon * 1e7).round();
      proto.add(
        FixWave(
          dLatE7: latE7 - prevLat,
          dLonE7: lonE7 - prevLon,
          dTsMs: f.tMs - prevTs,
          accuracyDm: 50,
          speedCms: 1500,
          bearingDeg: 45,
        ),
      );
      prevLat = latE7;
      prevLon = lonE7;
      prevTs = f.tMs;
    }
    channel.sink.add(
      PingBatchWave(
        sessionToken: sessionToken,
        seqStart: seqStart,
        datasetVersion: datasetVersion,
        fixes: proto,
      ).encode(),
    );
    _seq = seqStart + fixes.length;
    _status.add('sent ${fixes.length} fixes (seq $seqStart)');
  }

  Future<void> dispose() async {
    await _channel?.sink.close();
    await _status.close();
  }
}

/// Receives binary protobuf LiveFrame deltas from `/v1/live`.
class LiveFeedClient {
  WebSocketChannel? _channel;
  final StreamController<List<VehicleWave>> _vehicles =
      StreamController.broadcast();
  final StreamController<String> _status = StreamController.broadcast();

  LiveFeedClient();

  Stream<List<VehicleWave>> get vehicles => _vehicles.stream;
  Stream<String> get status => _status.stream;

  Future<void> connect() async {
    _channel = WebSocketChannel.connect(
      Uri.parse('${ApiConfig.wsBase}/v1/live'),
    );
    _status.add('connected');
    _channel!.stream.listen(
      (msg) {
        if (msg is List<int>) {
          try {
            _vehicles.add(decodeLiveFrame(msg));
          } catch (e) {
            _status.add('decode error: $e');
          }
        } else {
          _status.add('unexpected frame: ${msg.runtimeType}');
        }
      },
      onError: (e) => _status.add('error: $e'),
      onDone: () => _status.add('disconnected'),
    );
  }

  Future<void> dispose() async {
    await _channel?.sink.close();
    await _vehicles.close();
    await _status.close();
  }
}
