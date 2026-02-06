// Leave calculator (legacy)
export {
  loadHolidayCalendar,
  getHolidayCalendarForYear,
  calculateLeaveDays,
  formatLeaveDayTable,
  compareTripOptions,
  formatComparisonTable,
} from './leave-calculator';

export type {
  TripOption,
  TripComparison,
} from './leave-calculator';

// Holiday calculator (new)
export {
  calculateLeave,
  getHolidaysInRange,
  getDateRange,
  isHoliday,
  isWeekend,
  isWorkday,
  requiresLeave,
  getCalendar,
  clearCalendarCache,
} from './holiday-calculator';

export type {
  LeaveResult,
  DateInfo,
  HolidayCalendar,
  HolidayEntry,
  MakeupWorkday,
  DayDetail,
  LeaveDayResult,
} from './holiday-calculator';

// Flight normalizer
export {
  normalizeFlightData,
  scanFlightFiles,
  formatFlight,
  LCC_AIRLINES,
  FULL_SERVICE_AIRLINES,
} from './flight-normalizer';

export type {
  NormalizedFlight,
  FlightSearchResult,
} from './flight-normalizer';
