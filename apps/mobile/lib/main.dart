import 'package:flutter/material.dart';
import 'package:maplibre/maplibre.dart';
import 'package:provider/provider.dart';

import 'data/route.dart';
import 'state/tracker.dart';
import 'wire/pb.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const TabaaniApp());
}

class TabaaniApp extends StatelessWidget {
  const TabaaniApp({super.key});

  @override
  Widget build(BuildContext context) {
    return ChangeNotifierProvider(
      create: (_) {
        final state = TrackerState();
        state.connect();
        return state;
      },
      child: MaterialApp(
        title: 'Tabaani Live',
        debugShowCheckedModeBanner: false,
        theme: ThemeData(
          colorScheme: ColorScheme.fromSeed(
            seedColor: const Color(0xFFC0392B),
            brightness: Brightness.dark,
          ),
          useMaterial3: true,
          scaffoldBackgroundColor: const Color(0xFF0B0E14),
        ),
        home: const HomeScreen(),
      ),
    );
  }
}

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key});

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  int _tab = 0;

  @override
  Widget build(BuildContext context) {
    final state = context.watch<TrackerState>();
    return Scaffold(
      body: Column(
        children: [
          _Header(state: state),
          Expanded(
            child: IndexedStack(
              index: _tab,
              children: const [_MapTab(), _FeedTab(), _LogTab()],
            ),
          ),
          _BottomNav(index: _tab, onTap: (i) => setState(() => _tab = i)),
        ],
      ),
    );
  }
}

class _Header extends StatelessWidget {
  final TrackerState state;
  const _Header({required this.state});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 10),
      decoration: const BoxDecoration(
        color: Color(0xFF11151D),
        border: Border(bottom: BorderSide(color: Color(0xFF232A36))),
      ),
      child: Row(
        children: [
          Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              color: const Color(0xFFC0392B),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(
              Icons.train_rounded,
              color: Colors.white,
              size: 20,
            ),
          ),
          const SizedBox(width: 10),
          const Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'TABAANI',
                  style: TextStyle(
                    fontSize: 15,
                    fontWeight: FontWeight.w800,
                    letterSpacing: 1.4,
                  ),
                ),
                Text(
                  'TGM · Tunis–Marsa',
                  style: TextStyle(fontSize: 11, color: Color(0xFF8A95A5)),
                ),
              ],
            ),
          ),
          _Dot(ok: state.liveUp, label: 'LIVE'),
          const SizedBox(width: 8),
          _Dot(ok: state.ingestUp, label: 'UP'),
        ],
      ),
    );
  }
}

class _Dot extends StatelessWidget {
  final bool ok;
  final String label;
  const _Dot({required this.ok, required this.label});

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Container(
          width: 7,
          height: 7,
          decoration: BoxDecoration(
            color: ok ? Colors.greenAccent : Colors.redAccent,
            shape: BoxShape.circle,
          ),
        ),
        const SizedBox(width: 4),
        Text(
          label,
          style: const TextStyle(fontSize: 10, color: Color(0xFF8A95A5)),
        ),
      ],
    );
  }
}

class _MapTab extends StatelessWidget {
  const _MapTab();

  @override
  Widget build(BuildContext context) {
    final state = context.watch<TrackerState>();
    final routeLine = Feature<LineString>(
      geometry: LineString(
        PositionSeries.from(
          tgmRoute.map((p) => Geographic(lon: p.lon, lat: p.lat)),
        ),
      ),
    );
    final stopPoints = tgmStops
        .map(
          (s) => Feature<Point>(
            geometry: Point(Geographic(lon: s.lon, lat: s.lat)),
          ),
        )
        .toList();
    final vehiclePoints = state.vehicles
        .map(
          (v) => Feature<Point>(
            geometry: Point(Geographic(lon: 10.19, lat: 36.80)),
          ),
        )
        .toList();

    return Stack(
      children: [
        MapLibreMap(
          options: MapOptions(
            // The user-requested base style.
            style: MapStyle.url('https://styles.maptoolkit.org/dark.json'),
            initCamera: const .position(
              center: Geographic(lon: 10.29, lat: 36.84),
              zoom: 10.5,
            ),
          ),
          layers: [
            PolylineLayer(
              id: 'tgm-route',
              polylines: [routeLine],
              color: const .literal(Color(0xFFC0392B)),
              width: const .literal(4),
              opacity: const .literal(0.9),
            ),
            CircleLayer(
              id: 'tgm-stops',
              points: stopPoints,
              color: const .literal(Color(0xFFE8EDF5)),
              radius: const .literal(3.5),
              strokeColor: const .literal(Color(0xFF0B0E14)),
              strokeWidth: const .literal(1),
            ),
            CircleLayer(
              id: 'live-vehicles',
              points: vehiclePoints,
              color: const .literal(Color(0xFF2ECC71)),
              radius: const .literal(9),
              strokeColor: const .literal(Colors.white),
              strokeWidth: const .literal(2),
            ),
          ],
        ),
        Positioned(left: 12, bottom: 12, child: _RiderButton(state: state)),
        if (state.vehicles.isNotEmpty)
          Positioned(
            right: 12,
            top: 12,
            child: _LiveBadge(vehicles: state.vehicles),
          ),
      ],
    );
  }
}

class _RiderButton extends StatelessWidget {
  final TrackerState state;
  const _RiderButton({required this.state});

  @override
  Widget build(BuildContext context) {
    return ElevatedButton.icon(
      onPressed: state.toggleRider,
      icon: Icon(state.simulator.running ? Icons.stop : Icons.play_arrow),
      label: Text(state.simulator.running ? 'STOP RIDER' : 'START RIDER'),
      style: ElevatedButton.styleFrom(
        backgroundColor: state.simulator.running
            ? Colors.redAccent
            : const Color(0xFFC0392B),
        foregroundColor: Colors.white,
      ),
    );
  }
}

class _LiveBadge extends StatelessWidget {
  final List<VehicleWave> vehicles;
  const _LiveBadge({required this.vehicles});

  @override
  Widget build(BuildContext context) {
    final v = vehicles.first;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: const Color(0xFF11151D).withValues(alpha: 0.92),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: const Color(0xFF2ECC71)),
      ),
      child: Text(
        '${v.tripId.isEmpty ? v.routeId : v.tripId}  ·  '
        '${v.delayS.toStringAsFixed(0)}s  ·  ${v.confidenceLabel}',
        style: const TextStyle(fontSize: 11, color: Color(0xFF2ECC71)),
      ),
    );
  }
}

class _FeedTab extends StatelessWidget {
  const _FeedTab();

  @override
  Widget build(BuildContext context) {
    final state = context.watch<TrackerState>();
    if (state.vehicles.isEmpty) {
      return const Center(
        child: Text(
          'Waiting for live vehicles…',
          style: TextStyle(color: Color(0xFF8A95A5)),
        ),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.all(12),
      itemCount: state.vehicles.length,
      itemBuilder: (context, i) {
        final v = state.vehicles[i];
        return Card(
          color: const Color(0xFF151A23),
          child: ListTile(
            leading: const Icon(Icons.train_rounded, color: Color(0xFF2ECC71)),
            title: Text(
              v.tripId.isEmpty ? v.vehicleId : v.tripId,
              style: const TextStyle(fontWeight: FontWeight.bold),
            ),
            subtitle: Text(
              's=${v.sM.toStringAsFixed(0)}m  '
              'v=${v.vMps.toStringAsFixed(1)}m/s  riders=${v.nRiders}',
            ),
            trailing: Chip(
              label: Text(
                v.confidenceLabel,
                style: const TextStyle(fontSize: 10),
              ),
              backgroundColor: v.confidence == 3
                  ? const Color(0xFF1E4620)
                  : const Color(0xFF2A2F39),
            ),
          ),
        );
      },
    );
  }
}

class _LogTab extends StatelessWidget {
  const _LogTab();

  @override
  Widget build(BuildContext context) {
    final state = context.watch<TrackerState>();
    return ListView.builder(
      padding: const EdgeInsets.all(12),
      itemCount: state.log.length,
      itemBuilder: (context, i) => Padding(
        padding: const EdgeInsets.symmetric(vertical: 2),
        child: Text(
          state.log[i],
          style: const TextStyle(
            fontFamily: 'monospace',
            fontSize: 11,
            color: Color(0xFFB8C2D0),
          ),
        ),
      ),
    );
  }
}

class _BottomNav extends StatelessWidget {
  final int index;
  final ValueChanged<int> onTap;
  const _BottomNav({required this.index, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return NavigationBar(
      selectedIndex: index,
      onDestinationSelected: onTap,
      backgroundColor: const Color(0xFF11151D),
      destinations: const [
        NavigationDestination(icon: Icon(Icons.map), label: 'Map'),
        NavigationDestination(icon: Icon(Icons.departure_board), label: 'Live'),
        NavigationDestination(icon: Icon(Icons.terminal), label: 'Log'),
      ],
    );
  }
}
