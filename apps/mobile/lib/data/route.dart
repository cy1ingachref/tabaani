// Generated from data/gtfs/build/0.1.0-dev (do not edit by hand).
// Regenerate with: python tool/gen_route.py
library;

class RoutePoint {
  final double lat;
  final double lon;
  const RoutePoint(this.lat, this.lon);
}

class StopPoint {
  final String id;
  final String name;
  final double lat;
  final double lon;
  const StopPoint({
    required this.id,
    required this.name,
    required this.lat,
    required this.lon,
  });
}

/// The TGM northbound shape (lat, lon), used for the drawn line and rider sim.
const List<RoutePoint> tgmRoute = [
  RoutePoint(36.800690, 10.191740),
  RoutePoint(36.814180, 10.292450),
  RoutePoint(36.818050, 10.301940),
  RoutePoint(36.819840, 10.305600),
  RoutePoint(36.824200, 10.308770),
  RoutePoint(36.828780, 10.311550),
  RoutePoint(36.831950, 10.313870),
  RoutePoint(36.836170, 10.316610),
  RoutePoint(36.841540, 10.319100),
  RoutePoint(36.846130, 10.321850),
  RoutePoint(36.850120, 10.325580),
  RoutePoint(36.853390, 10.328920),
  RoutePoint(36.861650, 10.334940),
  RoutePoint(36.866590, 10.337440),
  RoutePoint(36.870450, 10.342130),
  RoutePoint(36.875740, 10.339670),
  RoutePoint(36.882980, 10.333490),
  RoutePoint(36.883040, 10.333450),
];

/// Named stops along the corridor.
const List<StopPoint> tgmStops = [
  StopPoint(
    id: 'tunis_marine_nord',
    name: 'TUNIS MARINE NORD',
    lat: 36.800690,
    lon: 10.191740,
  ),
  StopPoint(id: 'le_bac', name: 'LE BAC', lat: 36.814180, lon: 10.292450),
  StopPoint(
    id: 'la_goulette',
    name: 'LA GOULETTE',
    lat: 36.818050,
    lon: 10.301940,
  ),
  StopPoint(
    id: 'goulette_neuve',
    name: 'GOULETTE NEUVE',
    lat: 36.819840,
    lon: 10.305600,
  ),
  StopPoint(
    id: 'goulette_casino',
    name: 'GOULETTE CASINO',
    lat: 36.824200,
    lon: 10.308770,
  ),
  StopPoint(
    id: 'khiareddine',
    name: 'KHIAREDDINE',
    lat: 36.828780,
    lon: 10.311550,
  ),
  StopPoint(id: 'a_roport', name: 'AEROPORT', lat: 36.831950, lon: 10.313870),
  StopPoint(id: 'le_kram', name: 'LE KRAM', lat: 36.836170, lon: 10.316610),
  StopPoint(
    id: 'carthage_salambo',
    name: 'CARTHAGE SALAMBO',
    lat: 36.841540,
    lon: 10.319100,
  ),
  StopPoint(
    id: 'carthage_byrsa',
    name: 'CARTHAGE BYRSA',
    lat: 36.846130,
    lon: 10.321850,
  ),
  StopPoint(
    id: 'carthage_dermech',
    name: 'CARTHAGE DERMECH',
    lat: 36.850120,
    lon: 10.325580,
  ),
  StopPoint(
    id: 'carthage_hannibal',
    name: 'CARTHAGE HANNIBAL',
    lat: 36.853390,
    lon: 10.328920,
  ),
  StopPoint(
    id: 'carthage_pr_sidence',
    name: 'CARTHAGE PRESIDENCE',
    lat: 36.861650,
    lon: 10.334940,
  ),
  StopPoint(
    id: 'carthage_amilcar',
    name: 'CARTHAGE AMILCAR',
    lat: 36.866590,
    lon: 10.337440,
  ),
  StopPoint(
    id: 'sidi_bousaid',
    name: 'SIDI BOUSAID',
    lat: 36.870450,
    lon: 10.342130,
  ),
  StopPoint(
    id: 'sidi_dhrif',
    name: 'SIDI DHRIF',
    lat: 36.875740,
    lon: 10.339670,
  ),
  StopPoint(
    id: 'la_corniche',
    name: 'LA CORNICHE',
    lat: 36.882980,
    lon: 10.333490,
  ),
  StopPoint(
    id: 'marsa_plage',
    name: 'MARSA PLAGE',
    lat: 36.883040,
    lon: 10.333450,
  ),
];
