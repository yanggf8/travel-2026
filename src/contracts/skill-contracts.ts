/**
 * Skill Contracts v1.0.0
 *
 * Defines the interface for all CLI operations.
 * Agent can query this to discover available operations.
 *
 * Versioning: semver
 * - MAJOR: breaking changes to args/output shape
 * - MINOR: new operations or optional args
 * - PATCH: bug fixes, no interface change
 */

export const CONTRACT_VERSION = '1.0.0';

export interface SkillContract {
  name: string;
  description: string;
  args: {
    name: string;
    type: 'string' | 'number' | 'boolean' | 'string[]';
    required: boolean;
    description: string;
  }[];
  output: {
    type: 'void' | 'object' | 'array' | 'string';
    description: string;
  };
  mutates: string[];  // State keys this operation may change
  example: string;
}

/**
 * All available CLI operations with their contracts.
 * Agent should query this before invoking operations.
 */
export const SKILL_CONTRACTS: Record<string, SkillContract> = {
  'set-dates': {
    name: 'set-dates',
    description: 'Set travel dates. Triggers cascade to invalidate dependent processes.',
    args: [
      { name: 'start', type: 'string', required: true, description: 'Start date (YYYY-MM-DD)' },
      { name: 'end', type: 'string', required: true, description: 'End date (YYYY-MM-DD)' },
      { name: 'reason', type: 'string', required: false, description: 'Reason for change' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_1_date_anchor',
      'travel-plan.cascade_state',
      'state.event_log',
    ],
    example: 'npm run update set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"',
  },

  'select-offer': {
    name: 'select-offer',
    description: 'Select a package offer for booking. Populates P3/P4 from offer.',
    args: [
      { name: 'offer-id', type: 'string', required: true, description: 'Offer ID (e.g., besttour_TYO05MM260211AM)' },
      { name: 'date', type: 'string', required: true, description: 'Booking date (YYYY-MM-DD)' },
      { name: '--no-populate', type: 'boolean', required: false, description: 'Skip populating P3/P4' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_3_4_packages',
      'travel-plan.destinations.*.process_3_transportation',
      'travel-plan.destinations.*.process_4_accommodation',
      'state.event_log',
    ],
    example: 'npm run update select-offer besttour_TYO05MM260211AM 2026-02-13',
  },

  'mark-booked': {
    name: 'mark-booked',
    description: 'Mark package, flight, and hotel as booked after user confirms.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_3_4_packages.status',
      'travel-plan.destinations.*.process_3_transportation.status',
      'travel-plan.destinations.*.process_4_accommodation.status',
      'state.next_actions',
      'state.current_focus',
      'state.event_log',
    ],
    example: 'npm run update mark-booked',
  },

  'scaffold-itinerary': {
    name: 'scaffold-itinerary',
    description: 'Create day skeletons for P5 itinerary based on date anchor.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
      { name: '--force', type: 'boolean', required: false, description: 'Overwrite existing itinerary' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary',
      'state.event_log',
    ],
    example: 'npm run update -- scaffold-itinerary --force',
  },

  'populate-itinerary': {
    name: 'populate-itinerary',
    description: 'Populate itinerary sessions with activities from clusters.',
    args: [
      { name: '--goals', type: 'string', required: true, description: 'Comma-separated cluster names' },
      { name: '--pace', type: 'string', required: false, description: 'relaxed|balanced|packed (default: balanced)' },
      { name: '--assign', type: 'string', required: false, description: 'Manual cluster:day assignments' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug' },
      { name: '--force', type: 'boolean', required: false, description: 'Overwrite existing activities' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary.days',
      'state.event_log',
    ],
    example: 'npm run update -- populate-itinerary --goals "chanel_shopping,teamlab_roppongi" --pace balanced',
  },

  'status': {
    name: 'status',
    description: 'Show current plan status summary. Read-only.',
    args: [
      { name: '--full', type: 'boolean', required: false, description: 'Show flight/hotel details' },
    ],
    output: { type: 'string', description: 'Formatted status output' },
    mutates: [],  // Read-only
    example: 'npm run update:status',
  },

  'set-airport-transfer': {
    name: 'set-airport-transfer',
    description: 'Set airport transfer plan (selected + candidates) for arrival/departure.',
    args: [
      { name: 'direction', type: 'string', required: true, description: 'arrival | departure' },
      { name: 'status', type: 'string', required: true, description: 'planned | booked' },
      { name: '--selected', type: 'string', required: true, description: 'Pipe-delimited spec: title|route|duration_min?|price_yen?|schedule?' },
      { name: '--candidate', type: 'string', required: false, description: 'Repeatable pipe-delimited spec for candidates' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_3_transportation.airport_transfers',
      'state.event_log',
    ],
    example: 'npm run update -- set-airport-transfer arrival planned --selected "Limousine Bus|NRT T2 → Shiodome (Takeshiba)|85|3200|19:40 → ~21:05"',
  },

  'set-activity-booking': {
    name: 'set-activity-booking',
    description: 'Set booking status for an activity (tracks confirmation state).',
    args: [
      { name: 'day', type: 'number', required: true, description: 'Day number (1-indexed)' },
      { name: 'session', type: 'string', required: true, description: 'morning | afternoon | evening' },
      { name: 'activity', type: 'string', required: true, description: 'Activity ID or title (case-insensitive)' },
      { name: 'status', type: 'string', required: true, description: 'not_required | pending | booked | waitlist' },
      { name: '--ref', type: 'string', required: false, description: 'Booking reference/confirmation number' },
      { name: '--book-by', type: 'string', required: false, description: 'Deadline to book (YYYY-MM-DD)' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.booking_status',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.booking_ref',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.book_by',
      'state.event_log',
    ],
    example: 'npm run update -- set-activity-booking 3 morning "teamLab Borderless" booked --ref "TLB-12345"',
  },

  'update-offer': {
    name: 'update-offer',
    description: 'Update offer availability for a specific date.',
    args: [
      { name: 'offer-id', type: 'string', required: true, description: 'Offer ID' },
      { name: 'date', type: 'string', required: true, description: 'Date (YYYY-MM-DD)' },
      { name: 'availability', type: 'string', required: true, description: 'available|sold_out|limited' },
      { name: 'price', type: 'number', required: false, description: 'New price' },
      { name: 'seats', type: 'number', required: false, description: 'Seats remaining' },
      { name: 'source', type: 'string', required: false, description: 'Info source (agent|scrape|user)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_3_4_packages.results.offers',
      'state.event_log',
    ],
    example: 'npm run update update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent',
  },
};

/**
 * StateManager method contracts.
 * Documents all public methods for agent discovery.
 */
export const STATE_MANAGER_METHODS = {
  // Timestamp
  now: { returns: 'string', description: 'Get session timestamp' },
  freshTimestamp: { returns: 'string', description: 'Generate new timestamp' },
  refreshTimestamp: { returns: 'void', description: 'Refresh session timestamp' },

  // Dirty flags
  markDirty: { args: ['destination', 'process'], returns: 'void', description: 'Mark process dirty' },
  clearDirty: { args: ['destination', 'process'], returns: 'void', description: 'Clear dirty flag' },
  markGlobalDirty: { args: ['process'], returns: 'void', description: 'Mark global process dirty' },
  isDirty: { args: ['destination', 'process'], returns: 'boolean', description: 'Check if dirty' },

  // Process status
  setProcessStatus: { args: ['destination', 'process', 'status'], returns: 'void', description: 'Set process status' },
  getProcessStatus: { args: ['destination', 'process'], returns: 'ProcessStatus|null', description: 'Get process status' },

  // Events
  emitEvent: { args: ['event'], returns: 'void', description: 'Emit audit event' },
  getEventLog: { args: [], returns: 'TravelEvent[]', description: 'Get event log' },

  // Date anchor
  setDateAnchor: { args: ['startDate', 'endDate', 'reason?'], returns: 'void', description: 'Set travel dates' },
  getDateAnchor: { args: [], returns: '{ start, end, days }|null', description: 'Get travel dates' },

  // Offers
  updateOfferAvailability: { args: ['offerId', 'date', 'availability', 'price?', 'seats?', 'source?'], returns: 'void', description: 'Update offer availability' },
  selectOffer: { args: ['offerId', 'date', 'populateCascade?'], returns: 'void', description: 'Select offer for booking' },
  importPackageOffers: { args: ['destination', 'sourceId', 'offers', 'note?', 'warnings?'], returns: 'void', description: 'Import scraped offers' },

  // Itinerary
  scaffoldItinerary: { args: ['destination', 'days', 'force?'], returns: 'void', description: 'Create day skeletons' },
  addActivity: { args: ['destination', 'dayNumber', 'session', 'activity'], returns: 'string', description: 'Add activity, returns ID' },
  updateActivity: { args: ['destination', 'dayNumber', 'session', 'activityId', 'updates'], returns: 'void', description: 'Update activity' },
  removeActivity: { args: ['destination', 'dayNumber', 'session', 'activityId'], returns: 'void', description: 'Remove activity' },
  setDayTheme: { args: ['destination', 'dayNumber', 'theme'], returns: 'void', description: 'Set day theme' },
  setSessionFocus: { args: ['destination', 'dayNumber', 'session', 'focus'], returns: 'void', description: 'Set session focus' },
  setActivityBookingStatus: { args: ['destination', 'dayNumber', 'session', 'activityIdOrTitle', 'status', 'ref?', 'bookBy?'], returns: 'void', description: 'Set activity booking status' },
  findActivity: { args: ['destination', 'idOrTitle'], returns: '{ dayNumber, session, activity } | null', description: 'Find activity by ID or title' },

  // Airport transfers
  setAirportTransferSegment: { args: ['destination', 'direction', 'segment'], returns: 'void', description: 'Set airport transfer segment (arrival/departure)' },
  addAirportTransferCandidate: { args: ['destination', 'direction', 'option'], returns: 'void', description: 'Add candidate airport transfer option' },
  selectAirportTransferOption: { args: ['destination', 'direction', 'optionId'], returns: 'void', description: 'Select airport transfer option by ID' },

  // Destination
  getActiveDestination: { args: [], returns: 'string', description: 'Get active destination slug' },
  setActiveDestination: { args: ['destination'], returns: 'void', description: 'Set active destination' },
  setFocus: { args: ['destination', 'process'], returns: 'void', description: 'Set current focus' },
  setNextActions: { args: ['actions'], returns: 'void', description: 'Set next actions list' },
  getNextActions: { args: [], returns: 'string[]', description: 'Get next actions list' },

  // I/O
  save: { args: [], returns: 'void', description: 'Save both plan and state files' },
  getPlan: { args: [], returns: 'TravelPlanMinimal', description: 'Get current plan object' },
} as const;

/**
 * Get contract for a skill by name.
 */
export function getSkillContract(name: string): SkillContract | undefined {
  return SKILL_CONTRACTS[name];
}

/**
 * List all available skills.
 */
export function listSkills(): string[] {
  return Object.keys(SKILL_CONTRACTS);
}

/**
 * Validate that StateManager has all documented methods.
 * Call this at startup to catch interface drift.
 */
export function validateStateManagerInterface(sm: object): string[] {
  const missing: string[] = [];
  for (const method of Object.keys(STATE_MANAGER_METHODS)) {
    if (typeof (sm as Record<string, unknown>)[method] !== 'function') {
      missing.push(method);
    }
  }
  return missing;
}
