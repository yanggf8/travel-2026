"""
Tests for scrapers.schema — Unified output schema and validation.
"""

from scrapers.schema import (
    ScrapeResult, FlightInfo, FlightSegment, HotelInfo,
    PriceInfo, DatePricing, DatesInfo, ItineraryDay,
    validate_result,
)


class TestFlightSegment:
    def test_empty_is_not_populated(self):
        seg = FlightSegment()
        assert not seg.is_populated

    def test_populated_with_number_and_code(self):
        seg = FlightSegment(flight_number="MM620", departure_code="TPE")
        assert seg.is_populated

    def test_missing_code_not_populated(self):
        seg = FlightSegment(flight_number="MM620")
        assert not seg.is_populated


class TestFlightInfo:
    def test_to_dict_renames_return(self):
        fi = FlightInfo(
            outbound=FlightSegment(flight_number="MM620"),
            return_=FlightSegment(flight_number="MM627"),
        )
        d = fi.to_dict()
        assert "return" in d
        assert "return_" not in d
        assert d["return"]["flight_number"] == "MM627"

    def test_from_dict_reads_return_key(self):
        d = {
            "outbound": {"flight_number": "MM620", "departure_code": "TPE"},
            "return": {"flight_number": "MM627", "departure_code": "NRT"},
        }
        fi = FlightInfo.from_dict(d)
        assert fi.outbound.flight_number == "MM620"
        assert fi.return_.flight_number == "MM627"

    def test_from_dict_handles_return_underscore(self):
        d = {
            "outbound": {"flight_number": "X1"},
            "return_": {"flight_number": "X2"},
        }
        fi = FlightInfo.from_dict(d)
        assert fi.return_.flight_number == "X2"


class TestScrapeResult:
    def test_to_legacy_dict_structure(self):
        result = ScrapeResult(
            source_id="besttour",
            url="https://test.com",
            scraped_at="2026-02-06T00:00:00",
            title="Test Tour",
            flight=FlightInfo(
                outbound=FlightSegment(flight_number="MM620", departure_code="TPE", arrival_code="NRT"),
            ),
            price=PriceInfo(per_person=25000, currency="TWD"),
            inclusions=["breakfast"],
        )
        legacy = result.to_legacy_dict()

        assert legacy["url"] == "https://test.com"
        assert legacy["scraped_at"] == "2026-02-06T00:00:00"
        assert legacy["title"] == "Test Tour"
        assert "extracted" in legacy
        assert legacy["extracted"]["flight"]["outbound"]["flight_number"] == "MM620"
        assert legacy["extracted"]["price"]["per_person"] == 25000
        assert legacy["extracted"]["inclusions"] == ["breakfast"]

    def test_from_dict_roundtrip(self):
        original = {
            "url": "https://test.com",
            "scraped_at": "2026-02-06T00:00:00",
            "title": "Test",
            "raw_text": "some text",
            "extracted": {
                "flight": {
                    "outbound": {"flight_number": "IT200", "departure_code": "TPE", "arrival_code": "NRT"},
                    "return": {"flight_number": "IT201", "departure_code": "NRT", "arrival_code": "TPE"},
                },
                "hotel": {"name": "Hotel ABC", "names": ["Hotel ABC"]},
                "price": {"per_person": 30000, "currency": "TWD"},
                "dates": {"duration_days": 5, "duration_nights": 4},
                "inclusions": ["breakfast", "airport_tax"],
                "date_pricing": {
                    "2026-02-13": {"price": 27888, "availability": "available"},
                },
                "itinerary": [
                    {"day": 1, "content": "Arrival", "is_free": False, "is_guided": False},
                ],
            },
            "extracted_elements": {},
        }

        result = ScrapeResult.from_dict(original)
        assert result.flight.outbound.flight_number == "IT200"
        assert result.flight.return_.flight_number == "IT201"
        assert result.hotel.name == "Hotel ABC"
        assert result.price.per_person == 30000
        assert result.dates.duration_days == 5
        assert result.inclusions == ["breakfast", "airport_tax"]
        assert len(result.date_pricing) == 1
        assert result.date_pricing["2026-02-13"].price == 27888
        assert len(result.itinerary) == 1

    def test_from_dict_with_real_besttour(self, besttour_data):
        result = ScrapeResult.from_dict(besttour_data)
        assert result.flight.outbound.flight_number == "MM620"
        assert result.flight.return_.flight_number == "MM627"
        legacy = result.to_legacy_dict()
        assert legacy["extracted"]["flight"]["outbound"]["flight_number"] == "MM620"

    def test_from_dict_with_real_lifetour(self, lifetour_data):
        result = ScrapeResult.from_dict(lifetour_data)
        assert result.flight.outbound.flight_number == "D7378"
        assert result.price.per_person == 20999
        assert result.dates.duration_days == 5
        assert "travel_insurance" in result.inclusions


class TestValidateResult:
    def test_empty_result_has_warnings(self):
        result = ScrapeResult()
        warnings = validate_result(result)
        assert "Missing source_id" in warnings
        assert "Missing url" in warnings
        assert "No flight data extracted" in warnings
        assert "No hotel data extracted" in warnings
        assert "No price data extracted" in warnings

    def test_populated_result_fewer_warnings(self):
        result = ScrapeResult(
            source_id="besttour",
            url="https://test.com",
            scraped_at="2026-02-06T00:00:00",
            title="Test",
            flight=FlightInfo(
                outbound=FlightSegment(flight_number="MM620", departure_code="TPE", date="2026/02/13"),
            ),
            hotel=HotelInfo(name="Hotel X"),
            price=PriceInfo(per_person=25000),
        )
        warnings = validate_result(result)
        assert "Missing source_id" not in warnings
        assert "No flight data extracted" not in warnings
        assert "No hotel data extracted" not in warnings
        assert "No price data extracted" not in warnings

    def test_suspicious_price_warning(self):
        result = ScrapeResult(
            source_id="test",
            price=PriceInfo(per_person=100),
        )
        warnings = validate_result(result)
        assert any("suspiciously low" in w for w in warnings)
