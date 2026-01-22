/**
 * Types for process agent tools
 */

export interface ProcessContext {
  planPath: string;
  plan: TravelPlan;
}

export interface ProcessResult {
  success: boolean;
  processName: string;
  action: string;
  changes: string[];
  errors: string[];
}

export interface TravelPlan {
  project: string;
  version: string;
  currency: string;
  default_timezone: string;
  meta: any;
  readiness_rules: any;
  budget: Budget;
  process_1_date_anchor: DateAnchor;
  process_2_destination: Destination;
  process_3_transportation: Transportation;
  process_4_accommodation: Accommodation;
  process_5_daily_itinerary: DailyItinerary;
  schemas: Schemas;
}

export interface Budget {
  total_cap: number | null;
  flight_cap: number | null;
  accommodation_cap: number | null;
  daily_cap: number | null;
}

export interface DateAnchor {
  status: string;
  set_out_date: string;
  duration_days: number;
  return_date: string;
}

export interface Destination {
  status: string;
  primary_destination: string;
  country: string;
  region: string;
  sub_areas: string[];
}

export interface Transportation {
  status: string;
  flight: FlightInfo;
  home_to_airport: TransportLeg;
  airport_to_hotel: TransportLeg;
}

export interface FlightInfo {
  status: string;
  outbound: FlightLeg;
  return: FlightLeg;
  candidates: FlightCandidate[];
}

export interface FlightLeg {
  airline: string | null;
  flight_number: string | null;
  departure_airport: string | null;
  departure_airport_code: string | null;
  arrival_airport: string | null;
  arrival_airport_code: string | null;
  departure_datetime: string | null;
  departure_timezone: string | null;
  arrival_datetime: string | null;
  arrival_timezone: string | null;
  fare: number | null;
  fare_class: string | null;
  booking_ref: string | null;
  booking_url: string | null;
  notes: string | null;
}

export interface FlightCandidate {
  id: string;
  direction: 'outbound' | 'return';
  airline: string;
  flight_number: string;
  departure_airport: string;
  departure_airport_code: string;
  arrival_airport: string;
  arrival_airport_code: string;
  departure_datetime: string;
  departure_timezone: string | null;
  arrival_datetime: string;
  arrival_timezone: string | null;
  duration_minutes: number;
  stops: number;
  layover_airports: string[];
  fare: number;
  fare_class: string;
  baggage_included: string | null;
  refundable: boolean;
  booking_url: string | null;
  pros: string[];
  cons: string[];
}

export interface TransportLeg {
  status: string;
  method: string | null;
  route_description: string | null;
  departure_point: string | null;
  arrival_point: string | null;
  departure_time: string | null;
  duration_minutes: number | null;
  cost: number | null;
  booking_ref: string | null;
  candidates: TransportCandidate[];
  notes: string | null;
}

export interface TransportCandidate {
  id: string;
  method: string;
  operator: string | null;
  route_description: string;
  departure_point: string;
  arrival_point: string;
  departure_time: string | null;
  duration_minutes: number;
  cost: number;
  booking_required: boolean;
  booking_url: string | null;
  frequency: string | null;
  pros: string[];
  cons: string[];
}

export interface Accommodation {
  status: string;
  location_zone: LocationZone;
  hotel: HotelInfo;
}

export interface LocationZone {
  status: string;
  selected_area: string | null;
  selection_criteria: string | null;
  candidates: ZoneCandidate[];
}

export interface ZoneCandidate {
  id: string;
  name: string;
  description: string;
  pros: string[];
  cons: string[];
  main_stations: string[];
  hotel_price_range: { min: number | null; max: number | null };
  distance_to_center: string | null;
  vibe: string | null;
}

export interface HotelInfo {
  status: string;
  selected_hotel: string | null;
  candidates: HotelCandidate[];
  booking: HotelBooking;
}

export interface HotelCandidate {
  id: string;
  name: string;
  address: string;
  area: string;
  rating: number;
  review_count: number;
  price_per_night: number;
  total_price: number;
  amenities: string[];
  room_types: string[];
  distance_to_station: string | null;
  nearest_station: string | null;
  booking_url: string | null;
  cancellation_policy: string | null;
  pros: string[];
  cons: string[];
}

export interface HotelBooking {
  confirmation_number: string | null;
  check_in_date: string | null;
  check_in_time: string | null;
  check_out_date: string | null;
  check_out_time: string | null;
  room_type: string | null;
  total_price: number | null;
  price_per_night: number | null;
  cancellation_policy: string | null;
  cancellation_deadline: string | null;
  booking_url: string | null;
}

export interface DailyItinerary {
  status: string;
  days: DayPlan[];
}

export interface DayPlan {
  date: string;
  day_number: number;
  day_type: 'arrival' | 'full' | 'departure';
  status: string;
  morning: TimeSession;
  afternoon: TimeSession;
  evening: TimeSession;
}

export interface TimeSession {
  time_block: { start: string | null; end: string | null };
  activities: Activity[];
  notes: string | null;
}

export interface Activity {
  id: string;
  name: string;
  type: string | null;
  location: {
    name: string | null;
    address: string | null;
    coordinates: { lat: number | null; lng: number | null } | null;
    nearest_station: string | null;
    walking_minutes_from_station: number | null;
  };
  time: {
    start: string | null;
    end: string | null;
    duration_minutes: number | null;
    flexible: boolean;
  };
  cost: {
    admission: number | null;
    estimated_spending: number | null;
    currency: string;
  };
  booking: {
    required: boolean;
    booking_ref: string | null;
    booking_url: string | null;
    booked: boolean;
  };
  transit_to_next: {
    method: string | null;
    duration_minutes: number | null;
    cost: number | null;
    route: string | null;
  };
  notes: string | null;
  priority: string | null;
  weather_dependent: boolean;
}

export interface Schemas {
  flight_candidate: any;
  transport_candidate: any;
  zone_candidate: any;
  hotel_candidate: any;
  activity: any;
}
