"""
Tests for OTA parsers — pure parsing logic (no browser needed).

Uses real scraped data from data/ as fixtures.
"""

from scrapers.parsers.besttour import BestTourParser
from scrapers.parsers.lifetour import LifetourParser
from scrapers.parsers.settour import SettourParser
from scrapers.parsers.liontravel import LionTravelParser
from scrapers.parsers.tigerair import parse_tigerair_flights
from scrapers.parsers.trip_com import (
    parse_nonstop_flights, date_range, add_days, day_of_week, build_oneway_url,
)
from scrapers.parsers.google_flights import (
    GoogleFlightsParser, parse_flight_results, build_search_url as gf_build_url,
)
from scrapers.parsers.agoda import AgodaParser, build_hotel_url, CITY_IDS
from scrapers.registry import detect_ota, get_parser, get_available_parsers


class TestRegistry:
    def test_detect_ota_known_urls(self):
        assert detect_ota("https://www.besttour.com.tw/itinerary/X") == "besttour"
        assert detect_ota("https://vacation.liontravel.com/search") == "liontravel"
        assert detect_ota("https://tour.lifetour.com.tw/detail?x=1") == "lifetour"
        assert detect_ota("https://tour.settour.com.tw/product/X") == "settour"
        assert detect_ota("https://booking.tigerairtw.com/zh-TW/index") == "tigerair"
        assert detect_ota("https://www.trip.com/flights/taipei-to-osaka/") == "trip"
        assert detect_ota("https://www.google.com/travel/flights?q=test") == "google_flights"
        assert detect_ota("https://www.agoda.com/hotel/osaka") == "agoda"

    def test_detect_ota_unknown_url(self):
        assert detect_ota("https://www.google.com") is None
        assert detect_ota("https://example.com/besttour") is None

    def test_get_parser_all_sources(self):
        for sid in get_available_parsers():
            parser = get_parser(sid)
            assert parser.source_id == sid

    def test_get_parser_unknown_raises(self):
        import pytest
        with pytest.raises(ValueError, match="No parser registered"):
            get_parser("nonexistent_ota")

    def test_available_parsers(self):
        parsers = get_available_parsers()
        assert "besttour" in parsers
        assert "liontravel" in parsers
        assert "lifetour" in parsers
        assert "settour" in parsers
        assert "tigerair" in parsers
        assert "trip" in parsers


class TestBestTourParser:
    def test_parse_flights(self, besttour_data):
        parser = BestTourParser()
        result = parser.parse_raw_text(besttour_data["raw_text"], url=besttour_data["url"])

        assert result.flight.outbound.flight_number == "MM620"
        assert result.flight.outbound.departure_code == "TPE"
        assert result.flight.outbound.arrival_code == "NRT"
        assert result.flight.outbound.airline == "樂桃航空"

        assert result.flight.return_.flight_number == "MM627"
        assert result.flight.return_.departure_code == "NRT"
        assert result.flight.return_.arrival_code == "TPE"

    def test_parse_hotel(self, besttour_data):
        parser = BestTourParser()
        result = parser.parse_raw_text(besttour_data["raw_text"], url=besttour_data["url"])
        # Hotel may or may not be populated depending on page content
        # but should not error
        assert result.hotel is not None

    def test_parse_inclusions(self, besttour_data):
        parser = BestTourParser()
        result = parser.parse_raw_text(besttour_data["raw_text"], url=besttour_data["url"])
        assert isinstance(result.inclusions, list)

    def test_package_type_classification(self, besttour_data):
        parser = BestTourParser()
        result = parser.parse_raw_text(besttour_data["raw_text"], url=besttour_data["url"])
        # BestTour fixture is FIT (機加酒．東京自由行)
        assert result.package_type == "fit"

    def test_source_id(self):
        parser = BestTourParser()
        assert parser.source_id == "besttour"


class TestLifetourParser:
    def test_parse_flights(self, lifetour_data):
        parser = LifetourParser()
        result = parser.parse_raw_text(lifetour_data["raw_text"], url=lifetour_data["url"])

        assert result.flight.outbound.flight_number == "D7378"
        assert result.flight.outbound.departure_code == "TPE"
        assert result.flight.outbound.arrival_code == "OSA"
        assert result.flight.outbound.airline == "亞洲航空"

        assert result.flight.return_.flight_number == "D7379"
        assert result.flight.return_.departure_code == "OSA"
        assert result.flight.return_.arrival_code == "TPE"

    def test_parse_hotel(self, lifetour_data):
        parser = LifetourParser()
        result = parser.parse_raw_text(lifetour_data["raw_text"], url=lifetour_data["url"])

        assert len(result.hotel.names) > 0
        assert result.hotel.name == result.hotel.names[0]
        assert result.hotel.room_type == "SEMI DOUBLE"
        assert result.hotel.bed_width_cm == 140

    def test_parse_price(self, lifetour_data):
        parser = LifetourParser()
        result = parser.parse_raw_text(lifetour_data["raw_text"], url=lifetour_data["url"])

        assert result.price.per_person == 20999
        assert result.price.currency == "TWD"
        assert result.price.deposit == 10000
        assert result.price.seats_available == 23
        assert result.price.min_travelers == 16

    def test_parse_dates(self, lifetour_data):
        parser = LifetourParser()
        result = parser.parse_raw_text(lifetour_data["raw_text"], url=lifetour_data["url"])

        assert result.dates.duration_days == 5
        assert result.dates.duration_nights == 4
        assert result.dates.departure_month == 2
        assert result.dates.departure_day == 27
        assert result.dates.year == 2026
        # Check structured date
        assert result.dates.departure_date == "2026-02-27"
        assert result.dates.is_populated

    def test_package_type_classification(self, lifetour_data):
        parser = LifetourParser()
        result = parser.parse_raw_text(lifetour_data["raw_text"], url=lifetour_data["url"])
        # Lifetour fixture is "伴自由" (semi-guided) - classified as FIT for filtering
        assert result.package_type == "fit"

    def test_parse_itinerary(self, lifetour_data):
        parser = LifetourParser()
        result = parser.parse_raw_text(lifetour_data["raw_text"], url=lifetour_data["url"])

        assert len(result.itinerary) == 5
        assert result.itinerary[0].day == 1
        assert isinstance(result.itinerary[0].content, str)

    def test_parse_inclusions(self, lifetour_data):
        parser = LifetourParser()
        result = parser.parse_raw_text(lifetour_data["raw_text"], url=lifetour_data["url"])

        assert "travel_insurance" in result.inclusions
        assert "airport_tax" in result.inclusions
        assert "breakfast" in result.inclusions


class TestSettourParser:
    def test_parse_no_crash(self, settour_data):
        """Settour data may have empty extraction but should not crash."""
        parser = SettourParser()
        result = parser.parse_raw_text(settour_data["raw_text"], url=settour_data["url"])

        assert result.source_id == "settour"
        assert isinstance(result.flight.outbound, object)
        assert isinstance(result.hotel, object)
        assert isinstance(result.inclusions, list)


class TestLionTravelParser:
    def test_parse_search_prices(self, liontravel_data):
        """LionTravel parser extracts prices from search results."""
        parser = LionTravelParser()
        result = parser.parse_raw_text(liontravel_data["raw_text"], url=liontravel_data["url"])

        assert result.source_id == "liontravel"
        # Fixture has TWD 18,500, 19,800, 21,200 - should extract cheapest
        assert result.price.is_populated
        assert result.price.per_person == 18500
        assert result.price.currency == "TWD"

    def test_package_type_classification(self, liontravel_data):
        parser = LionTravelParser()
        result = parser.parse_raw_text(liontravel_data["raw_text"], url=liontravel_data["url"])
        # LionTravel vacation.liontravel.com is FIT only
        assert result.package_type == "fit"

    def test_source_id(self):
        parser = LionTravelParser()
        assert parser.source_id == "liontravel"


class TestTigerairParser:
    def test_parse_empty_text(self):
        flights = parse_tigerair_flights("")
        assert flights == []

    def test_parse_flight_number(self):
        text = "IT200\n10:30\n14:00\nTWD 3,500\n2h 30m"
        flights = parse_tigerair_flights(text)
        assert len(flights) == 1
        assert flights[0]["flight_number"] == "IT200"
        assert flights[0]["departure_time"] == "10:30"
        assert flights[0]["arrival_time"] == "14:00"
        assert flights[0]["price"] == 3500

    def test_parse_multiple_flights(self):
        text = "IT200\n10:30\n14:00\nTWD 3,500\nIT202\n15:00\n19:00\nTWD 4,200"
        flights = parse_tigerair_flights(text)
        assert len(flights) == 2
        assert flights[0]["flight_number"] == "IT200"
        assert flights[1]["flight_number"] == "IT202"


class TestTripComParser:
    def test_date_range(self):
        dates = date_range("2026-02-24", "2026-02-27")
        assert dates == ["2026-02-24", "2026-02-25", "2026-02-26", "2026-02-27"]

    def test_date_range_single_day(self):
        dates = date_range("2026-03-01", "2026-03-01")
        assert dates == ["2026-03-01"]

    def test_add_days(self):
        assert add_days("2026-02-26", 5) == "2026-03-03"
        assert add_days("2026-02-28", 1) == "2026-03-01"

    def test_day_of_week(self):
        # 2026-02-06 is a Friday
        assert day_of_week("2026-02-06") == "Fri"

    def test_build_oneway_url(self):
        url = build_oneway_url("tpe", "kix", "2026-02-26", 2)
        assert "trip.com/flights/taipei-to-osaka/" in url
        assert "ddate=2026-02-26" in url
        assert "quantity=2" in url
        assert "flighttype=ow" in url

    def test_parse_nonstop_flights_empty(self):
        flights = parse_nonstop_flights("", pax=2)
        assert flights == []

    def test_parse_nonstop_flights_sample(self):
        # Simulate Trip.com output structure
        text = """Some Airline
8:30
TPE
3h 0m
Nonstop
11:30
KIX
US$140
Total US$280"""
        flights = parse_nonstop_flights(text, pax=2)
        assert len(flights) == 1
        assert flights[0]["nonstop"] is True
        assert flights[0]["price_per_person_usd"] == 140
        assert flights[0]["total_usd"] == 280


class TestGoogleFlightsParser:
    SAMPLE_TEXT = """航班搜尋
來回
找到 3 項結果。
凌晨2:30
 – 
清晨6:00
捷星日本航空
2 小時 30 分鐘
TPE–KIX
直達
146 公斤 CO2e
平均排放量
$12,528
來回票價
凌晨1:45
 – 
清晨5:10
樂桃航空
2 小時 25 分鐘
TPE–KIX
直達
144 公斤 CO2e
$14,742
來回票價
下午3:40
 – 
晚上7:20
亞洲航空 X
2 小時 40 分鐘
TPE–KIX
直達
121 公斤 CO2e
$15,990
來回票價"""

    def test_parse_flight_results(self):
        flights = parse_flight_results(self.SAMPLE_TEXT)
        assert len(flights) == 3
        assert flights[0]["price"] == 12528
        assert flights[0]["airline"] == "捷星日本航空"
        assert flights[0]["departure_time"] == "2:30"
        assert flights[0]["arrival_time"] == "6:00"
        assert flights[0]["nonstop"] is True
        assert flights[0]["departure_code"] == "TPE"
        assert flights[0]["arrival_code"] == "KIX"
        assert flights[0]["duration_minutes"] == 150

    def test_parse_multiple_airlines(self):
        flights = parse_flight_results(self.SAMPLE_TEXT)
        airlines = [f["airline"] for f in flights]
        assert "樂桃航空" in airlines
        assert "亞洲航空 X" in airlines

    def test_parse_empty_text(self):
        flights = parse_flight_results("")
        assert flights == []

    def test_build_search_url_roundtrip(self):
        url = gf_build_url("TPE", "KIX", "2026-02-26", "2026-03-02")
        assert "Flights%20to%20KIX%20from%20TPE" in url
        assert "2026-02-26" in url
        assert "2026-03-02" in url
        assert "curr=TWD" in url

    def test_build_search_url_oneway(self):
        url = gf_build_url("TPE", "NRT", "2026-02-26")
        assert "one%20way" in url
        assert "through" not in url

    def test_full_parser(self):
        parser = GoogleFlightsParser()
        result = parser.parse_raw_text(self.SAMPLE_TEXT)
        assert result.source_id == "google_flights"
        assert result.price.per_person == 12528
        assert result.flight.outbound.departure_time == "2:30"
        assert result.flight.outbound.arrival_time == "6:00"
        assert result.flight.outbound.departure_code == "TPE"
        assert result.flight.outbound.arrival_code == "KIX"
        assert len(result.extracted_elements.get("all_flights", [])) == 3


class TestAgodaParser:
    SAMPLE_TEXT = """大阪十字飯店 (Cross Hotel Osaka)
滿分5顆星，住宿獲得4顆星
5-15, 2 Chome, Shinsaibashi-Suji, Chuo-Ku, 心齋橋, 大阪, 日本, 542-0085
免費Wi-Fi
餐廳
早餐
停車場
溫泉
每晚只要
NT$ 3,706 起
客房選擇
高級雙人房
NT$ 3,706
豪華雙人房
NT$ 4,200"""

    HOTEL_URL = "https://www.agoda.com/cross-hotel-osaka/hotel/osaka-jp.html?checkIn=2026-02-26&los=4&adults=2"

    def test_parse_hotel_name(self):
        parser = AgodaParser()
        result = parser.parse_raw_text(self.SAMPLE_TEXT, url=self.HOTEL_URL)
        assert result.hotel.name == "大阪十字飯店"
        assert "Cross Hotel Osaka" in result.hotel.names

    def test_parse_star_rating(self):
        parser = AgodaParser()
        result = parser.parse_raw_text(self.SAMPLE_TEXT, url=self.HOTEL_URL)
        assert result.hotel.star_rating == 4

    def test_parse_price(self):
        parser = AgodaParser()
        result = parser.parse_raw_text(self.SAMPLE_TEXT, url=self.HOTEL_URL)
        assert result.price.per_person == 3706
        assert result.price.currency == "TWD"

    def test_parse_amenities(self):
        parser = AgodaParser()
        result = parser.parse_raw_text(self.SAMPLE_TEXT, url=self.HOTEL_URL)
        assert "免費Wi-Fi" in result.hotel.amenities
        assert "餐廳" in result.hotel.amenities
        assert "早餐" in result.hotel.amenities

    def test_parse_area(self):
        parser = AgodaParser()
        result = parser.parse_raw_text(self.SAMPLE_TEXT, url=self.HOTEL_URL)
        assert "心齋橋" in result.hotel.area

    def test_parse_dates_from_url(self):
        parser = AgodaParser()
        result = parser.parse_raw_text(self.SAMPLE_TEXT, url=self.HOTEL_URL)
        assert result.dates.departure_date == "2026-02-26"
        assert result.dates.duration_nights == 4

    def test_build_hotel_url(self):
        url = build_hotel_url("test-hotel", "osaka", check_in="2026-02-26", nights=3)
        assert "test-hotel" in url
        assert "osaka-jp" in url
        assert "checkIn=2026-02-26" in url
        assert "los=3" in url

    def test_city_ids(self):
        assert CITY_IDS["osaka"] == 14811
        assert CITY_IDS["tokyo"] == 5765
        assert "kyoto" in CITY_IDS

    def test_parse_empty_text(self):
        parser = AgodaParser()
        result = parser.parse_raw_text("", url="")
        assert result.hotel.name == ""
        assert result.price.per_person is None
