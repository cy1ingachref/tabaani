import 'package:flutter_test/flutter_test.dart';

import 'package:tabaani/wire/pb.dart';

void main() {
  test('protobuf codec round-trips a delta fix', () {
    final batch = PingBatchWave(
      sessionToken: 'test-session',
      seqStart: 7,
      datasetVersion: '0.1.0-dev',
      fixes: [
        FixWave(dLatE7: 1234567, dLonE7: -987654, dTsMs: 250, speedCms: 1500),
      ],
    );
    final encoded = batch.encode();
    final reader = PbReader(encoded);
    var found = false;
    while (!reader.done) {
      final (field, wt) = reader.tag();
      if (field == 4 && wt == 2) {
        final fix = PbReader(reader.len());
        while (!fix.done) {
          final (f, w) = fix.tag();
          switch (f) {
            case 1:
              expect(w, 0);
              expect(fix.sint32(), 1234567);
            case 2:
              expect(fix.sint32(), -987654);
            case 3:
              expect(fix.varint(), 250);
            case 5:
              expect(fix.varint(), 1500);
            default:
              fix.skip(w);
          }
        }
        found = true;
      } else {
        reader.skip(wt);
      }
    }
    expect(found, isTrue);
  });

  test('confidence label matches server enum', () {
    final v = VehicleWave()..confidence = 3;
    expect(v.confidenceLabel, 'LIVE');
  });
}
