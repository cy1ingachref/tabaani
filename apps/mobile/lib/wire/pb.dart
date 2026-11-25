/// Hand-rolled protobuf wire codec matching `proto/tabaani/v1/transit.proto`.
///
/// The server deliberately uses tiny messages and a hand-written decoder
/// (services/api/src/pb.rs). This Dart mirror implements the same field numbers
/// and wire types so wire format agrees without generating code.
library;

import 'dart:convert' show utf8;
import 'dart:typed_data' show ByteData, Endian, Uint8List;

class PbWriter {
  final List<int> _out = <int>[];

  List<int> get bytes => List<int>.unmodifiable(_out);

  void varint(int value) {
    var n = value;
    while (true) {
      var b = n & 0x7f;
      n >>= 7;
      if (n != 0) b |= 0x80;
      _out.add(b);
      if (n == 0) break;
    }
  }

  void tag(int field, int wireType) => varint((field << 3) | wireType);

  void uint32(int field, int value) {
    if (value == 0) return;
    tag(field, 0);
    varint(value);
  }

  void sint32(int field, int value) {
    tag(field, 0);
    varint(((value << 1) ^ (value >> 31)) & 0xffffffff);
  }

  void fixed32(int field, double value) {
    if (value == 0.0) return;
    tag(field, 5);
    // ignore: avoid_slow_async_io
    final bits = _floatBits(value);
    _out.addAll(<int>[
      bits & 0xff,
      (bits >> 8) & 0xff,
      (bits >> 16) & 0xff,
      (bits >> 24) & 0xff,
    ]);
  }

  void string(int field, String value) {
    if (value.isEmpty) return;
    tag(field, 2);
    final data = utf8.encode(value);
    varint(data.length);
    _out.addAll(data);
  }

  void lengthDelimited(int field, List<int> data) {
    tag(field, 2);
    varint(data.length);
    _out.addAll(data);
  }
}

/// Reinterpret the low 32 bits of a double as an IEEE-754 float bit pattern.
int _floatBits(double v) {
  final b = ByteData(4)..setFloat32(0, v, Endian.little);
  return b.getUint32(0, Endian.little);
}

class PbReader {
  final List<int> _buf;
  int _pos = 0;

  PbReader(this._buf);

  bool get done => _pos >= _buf.length;

  int byte() {
    if (_pos >= _buf.length) throw const FormatException('truncated');
    return _buf[_pos++];
  }

  int varint() {
    var out = 0;
    var shift = 0;
    while (true) {
      final b = byte();
      out |= (b & 0x7f) << shift;
      if (b & 0x80 == 0) return out;
      shift += 7;
      if (shift >= 64) throw const FormatException('varint too long');
    }
  }

  (int, int) tag() {
    final key = varint();
    return (key >> 3, key & 0x07);
  }

  List<int> len() {
    final n = varint();
    if (_pos + n > _buf.length) throw const FormatException('bad length');
    final out = _buf.sublist(_pos, _pos + n);
    _pos += n;
    return out;
  }

  int sint32() {
    final n = varint() & 0xffffffff;
    return (n >> 1) ^ -(n & 1);
  }

  double fixed32() {
    if (_pos + 4 > _buf.length) throw const FormatException('truncated');
    final b = ByteData.sublistView(Uint8List.fromList(_buf), _pos, _pos + 4);
    _pos += 4;
    return b.getFloat32(0, Endian.little);
  }

  void skip(int wireType) {
    switch (wireType) {
      case 0:
        varint();
      case 1:
        _pos += 8;
      case 2:
        len();
      case 5:
        _pos += 4;
      default:
        throw FormatException('wire type $wireType');
    }
    if (_pos > _buf.length) throw const FormatException('truncated');
  }
}

// --- wire models -----------------------------------------------------------

class FixWave {
  final int dLatE7;
  final int dLonE7;
  final int dTsMs;
  final int accuracyDm;
  final int speedCms;
  final int bearingDeg;

  const FixWave({
    required this.dLatE7,
    required this.dLonE7,
    required this.dTsMs,
    this.accuracyDm = 50,
    this.speedCms = 0,
    this.bearingDeg = 0,
  });

  List<int> encode() {
    final w = PbWriter();
    w.sint32(1, dLatE7);
    w.sint32(2, dLonE7);
    w.uint32(3, dTsMs);
    w.uint32(4, accuracyDm);
    w.uint32(5, speedCms);
    w.uint32(6, bearingDeg);
    return w.bytes;
  }
}

class PingBatchWave {
  final String sessionToken;
  final int seqStart;
  final String datasetVersion;
  final List<FixWave> fixes;

  const PingBatchWave({
    required this.sessionToken,
    required this.seqStart,
    required this.datasetVersion,
    required this.fixes,
  });

  List<int> encode() {
    final w = PbWriter();
    w.string(1, sessionToken);
    w.uint32(2, seqStart);
    w.string(3, datasetVersion);
    for (final f in fixes) {
      w.lengthDelimited(4, f.encode());
    }
    return w.bytes;
  }
}

class VehicleWave {
  String vehicleId = '';
  String routeId = '';
  int direction = 0;
  double sM = 0;
  double vMps = 0;
  int tsMs = 0;
  int confidence = 0;
  int nRiders = 0;
  String tripId = '';
  double delayS = 0;
  String vehicleType = '';
  int nCars = 0;

  String get confidenceLabel => switch (confidence) {
    1 => 'Scheduled',
    2 => 'Estimated',
    3 => 'LIVE',
    _ => 'Unknown',
  };

  static VehicleWave decode(List<int> data) {
    final v = VehicleWave();
    final r = PbReader(data);
    while (!r.done) {
      final (field, wt) = r.tag();
      switch (field) {
        case 1 when wt == 2:
          v.vehicleId = utf8.decode(r.len());
        case 2 when wt == 2:
          v.routeId = utf8.decode(r.len());
        case 3 when wt == 0:
          v.direction = r.varint();
        case 4 when wt == 5:
          v.sM = r.fixed32();
        case 5 when wt == 5:
          v.vMps = r.fixed32();
        case 6 when wt == 0:
          v.tsMs = r.varint();
        case 7 when wt == 0:
          v.confidence = r.varint();
        case 8 when wt == 0:
          v.nRiders = r.varint();
        case 9 when wt == 2:
          v.tripId = utf8.decode(r.len());
        case 10 when wt == 5:
          v.delayS = r.fixed32();
        case 11 when wt == 2:
          v.vehicleType = utf8.decode(r.len());
        case 12 when wt == 0:
          v.nCars = r.varint();
        default:
          r.skip(wt);
      }
    }
    return v;
  }
}

List<VehicleWave> decodeLiveFrame(List<int> data) {
  final out = <VehicleWave>[];
  final r = PbReader(data);
  while (!r.done) {
    final (field, wt) = r.tag();
    if (field == 1 && wt == 2) {
      out.add(VehicleWave.decode(r.len()));
    } else {
      r.skip(wt);
    }
  }
  return out;
}
