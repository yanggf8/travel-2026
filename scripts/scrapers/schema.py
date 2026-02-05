"""
Unified Scraper Output Schema

Defines the canonical output format for all OTA scrapers.
Mirrors the TypeScript CanonicalOffer type in src/scrapers/types.ts.
"""

from __future__ import annotations

from dataclasses import dataclass, field, asdict
from datetime import datetime
from typing import Any, Optional


@dataclass
class FlightSegment:
    """A single flight leg (outbound or return)."""

    flight_number: str = ""
    airline: str = ""
    airline_code: str = ""
    departure_airport: str = ""
    departure_code: str = ""
    departure_time: str = ""
    arrival_airport: str = ""
    arrival_code: str = ""
    arrival_time: str = ""
    date: str = ""

    @property
    def is_populated(self) -> bool:
        """True if at least flight number and one airport code are present."""
        return bool(self.flight_number and (self.departure_code or self.arrival_code))


@dataclass
class FlightInfo:
    """Outbound + return flight pair."""

    outbound: FlightSegment = field(default_factory=FlightSegment)
    return_: FlightSegment = field(default_factory=FlightSegment)

    @property
    def is_populated(self) -> bool:
        return self.outbound.is_populated or self.return_.is_populated

    def to_dict(self) -> dict:
        d = asdict(self)
        # Rename return_ → return for JSON output
        d["return"] = d.pop("return_")
        return d

    @classmethod
    def from_dict(cls, data: dict) -> FlightInfo:
        outbound_data = data.get("outbound", {})
        return_data = data.get("return", data.get("return_", {}))
        return cls(
            outbound=FlightSegment(**{k: v for k, v in outbound_data.items() if k in FlightSegment.__dataclass_fields__}),
            return_=FlightSegment(**{k: v for k, v in return_data.items() if k in FlightSegment.__dataclass_fields__}),
        )


@dataclass
class HotelInfo:
    """Hotel details."""

    name: str = ""
    names: list[str] = field(default_factory=list)
    area: str = ""
    star_rating: Optional[int] = None
    access: list[str] = field(default_factory=list)
    amenities: list[str] = field(default_factory=list)
    room_type: str = ""
    bed_width_cm: Optional[int] = None

    @property
    def is_populated(self) -> bool:
        return bool(self.name or self.names)


@dataclass
class PriceInfo:
    """Pricing details."""

    per_person: Optional[int] = None
    total: Optional[int] = None
    currency: str = "TWD"
    deposit: Optional[int] = None
    seats_available: Optional[int] = None
    min_travelers: Optional[int] = None

    @property
    def is_populated(self) -> bool:
        return self.per_person is not None


@dataclass
class DatePricing:
    """Price for a specific departure date."""

    date: str = ""
    price: Optional[int] = None
    availability: str = "unknown"  # available | sold_out | limited | unknown
    seats_remaining: Optional[int] = None
    notes: str = ""


@dataclass
class DatesInfo:
    """Travel date metadata."""

    duration_days: Optional[int] = None
    duration_nights: Optional[int] = None
    departure_date: str = ""  # ISO format: YYYY-MM-DD
    return_date: str = ""     # ISO format: YYYY-MM-DD
    year: Optional[int] = None
    departure_month: Optional[int] = None
    departure_day: Optional[int] = None
    
    @property
    def is_populated(self) -> bool:
        """True if at least departure_date is set."""
        return bool(self.departure_date)


@dataclass
class ItineraryDay:
    """A single day in the itinerary."""

    day: int = 0
    content: str = ""
    is_free: bool = False
    is_guided: bool = False


@dataclass
class ScrapeResult:
    """
    Canonical scraper output.

    This is the unified output format for all OTA scrapers.
    Mirrors CanonicalOffer + ScrapeResult from src/scrapers/types.ts.
    """

    # Provenance
    source_id: str = ""
    url: str = ""
    scraped_at: str = ""
    title: str = ""
    
    # Classification
    package_type: str = "unknown"  # "fit" | "group" | "flight" | "hotel" | "unknown"

    # Structured data
    flight: FlightInfo = field(default_factory=FlightInfo)
    hotel: HotelInfo = field(default_factory=HotelInfo)
    price: PriceInfo = field(default_factory=PriceInfo)
    dates: DatesInfo = field(default_factory=DatesInfo)
    inclusions: list[str] = field(default_factory=list)
    date_pricing: dict[str, DatePricing] = field(default_factory=dict)
    itinerary: list[ItineraryDay] = field(default_factory=list)

    # Package links (from listing pages)
    package_links: list[dict] = field(default_factory=list)

    # Raw data (for fallback / debugging)
    raw_text: str = ""
    extracted_elements: dict[str, list[str]] = field(default_factory=dict)

    # Result metadata
    success: bool = True
    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        """Serialize to dict, with flight.return_ → flight.return rename."""
        d = asdict(self)
        # Fix flight return_ key
        if "flight" in d:
            d["flight"]["return"] = d["flight"].pop("return_")
        # Convert date_pricing DatePricing objects to plain dicts
        if "date_pricing" in d:
            d["date_pricing"] = {
                k: {kk: vv for kk, vv in v.items() if vv is not None and vv != ""}
                for k, v in d["date_pricing"].items()
            }
        # Convert itinerary
        if "itinerary" in d:
            d["itinerary"] = [
                {k: v for k, v in day.items() if v is not None and v != "" and v != 0 and v is not False}
                if not day.get("is_free") and not day.get("is_guided")
                else day
                for day in d["itinerary"]
            ]
        return d

    @classmethod
    def from_dict(cls, data: dict) -> ScrapeResult:
        """Deserialize from dict (supports both to_dict() and to_legacy_dict() formats)."""
        result = cls()
        result.source_id = data.get("source_id", data.get("source", ""))
        result.package_type = data.get("package_type", "unknown")  # NEW: Restore package_type
        result.url = data.get("url", "")
        result.scraped_at = data.get("scraped_at", "")
        result.title = data.get("title", "")
        result.raw_text = data.get("raw_text", "")
        result.extracted_elements = data.get("extracted_elements", {})
        result.package_links = data.get("package_links", [])
        result.errors = data.get("errors", [])
        result.warnings = data.get("warnings", [])
        result.success = data.get("success", True)

        # Check if data is in new format (direct fields) or legacy format (extracted wrapper)
        extracted = data.get("extracted", {})
        use_legacy = bool(extracted)
        
        # Flight data
        flight_data = extracted.get("flight", {}) if use_legacy else data.get("flight", {})
        if flight_data:
            result.flight = FlightInfo.from_dict(flight_data)

        # Hotel data
        hotel_data = extracted.get("hotel", {}) if use_legacy else data.get("hotel", {})
        if hotel_data:
            result.hotel = HotelInfo(
                name=hotel_data.get("name", ""),
                names=hotel_data.get("names", []),
                area=hotel_data.get("area", ""),
                star_rating=hotel_data.get("star_rating"),
                access=hotel_data.get("access", []),
                amenities=hotel_data.get("amenities", []),
                room_type=hotel_data.get("room_type", ""),
                bed_width_cm=hotel_data.get("bed_width_cm"),
            )

        # Price data
        price_data = extracted.get("price", {}) if use_legacy else data.get("price", {})
        if price_data:
            result.price = PriceInfo(
                per_person=price_data.get("per_person"),
                total=price_data.get("total"),
                currency=price_data.get("currency", "TWD"),
                deposit=price_data.get("deposit"),
                seats_available=price_data.get("seats_available"),
                min_travelers=price_data.get("min_travelers"),
            )

        # Dates data
        dates_data = extracted.get("dates", {}) if use_legacy else data.get("dates", {})
        if dates_data:
            result.dates = DatesInfo(
                duration_days=dates_data.get("duration_days"),
                duration_nights=dates_data.get("duration_nights"),
                departure_date=dates_data.get("departure_date", ""),
                return_date=dates_data.get("return_date", ""),
                year=dates_data.get("year"),
                departure_month=dates_data.get("departure_month"),
                departure_day=dates_data.get("departure_day"),
            )

        # Inclusions
        result.inclusions = extracted.get("inclusions", []) if use_legacy else data.get("inclusions", [])

        # Date pricing
        dp_data = extracted.get("date_pricing", {}) if use_legacy else data.get("date_pricing", {})
        if dp_data:
            result.date_pricing = {
                k: DatePricing(
                    date=k,
                    price=v.get("price"),
                    availability=v.get("availability", "unknown"),
                    seats_remaining=v.get("seats_remaining"),
                    notes=v.get("notes", ""),
                )
                for k, v in dp_data.items()
            }

        # Itinerary
        itin_data = extracted.get("itinerary", []) if use_legacy else data.get("itinerary", [])
        if itin_data:
            result.itinerary = [
                ItineraryDay(
                    day=d.get("day", 0),
                    content=d.get("content", ""),
                    is_free=d.get("is_free", False),
                    is_guided=d.get("is_guided", False),
                )
                for d in itin_data
            ]

        return result

    def to_legacy_dict(self) -> dict:
        """
        Serialize to the legacy output format used by scrape_package.py.
        Preserves backward compatibility with existing data consumers.
        """
        extracted = {
            "flight": self.flight.to_dict(),
            "hotel": asdict(self.hotel),
            "price": {k: v for k, v in asdict(self.price).items() if v is not None},
            "dates": {k: v for k, v in asdict(self.dates).items() if v is not None},
            "inclusions": self.inclusions,
            "date_pricing": {
                k: {kk: vv for kk, vv in asdict(v).items() if vv is not None and kk != "date" and vv != ""}
                for k, v in self.date_pricing.items()
            },
        }
        if self.itinerary:
            extracted["itinerary"] = [asdict(d) for d in self.itinerary]

        result = {
            "source_id": self.source_id,  # NEW: Include source_id
            "package_type": self.package_type,  # NEW: Include package_type
            "url": self.url,
            "scraped_at": self.scraped_at,
            "title": self.title,
            "raw_text": self.raw_text,
            "extracted": extracted,
            "extracted_elements": self.extracted_elements,
        }
        if self.package_links:
            result["package_links"] = self.package_links
        return result


def validate_result(result: ScrapeResult) -> list[str]:
    """
    Validate a ScrapeResult and return a list of warnings.

    Does not raise — scraping is inherently lossy, so we report
    what's missing rather than failing hard.
    """
    warnings = []

    if not result.source_id:
        warnings.append("Missing source_id")
    if not result.url:
        warnings.append("Missing url")
    if not result.scraped_at:
        warnings.append("Missing scraped_at timestamp")
    if not result.title:
        warnings.append("Missing page title")

    # Flight validation
    if not result.flight.is_populated:
        warnings.append("No flight data extracted")
    else:
        if result.flight.outbound.is_populated and not result.flight.outbound.date:
            warnings.append("Outbound flight missing date")
        if result.flight.return_.is_populated and not result.flight.return_.date:
            warnings.append("Return flight missing date")

    # Hotel validation
    if not result.hotel.is_populated:
        warnings.append("No hotel data extracted")

    # Price validation
    if not result.price.is_populated:
        warnings.append("No price data extracted")
    elif result.price.per_person is not None and result.price.per_person < 5000:
        warnings.append(f"Price suspiciously low: {result.price.per_person}")

    # Date pricing
    if result.date_pricing:
        for date_key, dp in result.date_pricing.items():
            if dp.price is not None and dp.price < 1000:
                warnings.append(f"Date pricing for {date_key} suspiciously low: {dp.price}")

    return warnings
