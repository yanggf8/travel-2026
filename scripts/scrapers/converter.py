"""
Python → TypeScript Schema Converter

Converts Python ScrapeResult (snake_case) to TypeScript CanonicalOffer (camelCase).
Handles field name differences and data structure transformations.
"""

from __future__ import annotations

from typing import Any
from .schema import ScrapeResult


def to_camel_case(snake_str: str) -> str:
    """Convert snake_case to camelCase."""
    components = snake_str.split("_")
    return components[0] + "".join(x.title() for x in components[1:])


def convert_flight_segment(segment: dict) -> dict:
    """Convert FlightSegment from Python to TypeScript format."""
    return {
        "flightNumber": segment.get("flight_number", ""),
        "airline": segment.get("airline", ""),
        "airlineCode": segment.get("airline_code", ""),
        "departureAirport": segment.get("departure_airport", ""),
        "departureCode": segment.get("departure_code", ""),
        "departureTime": segment.get("departure_time", ""),
        "arrivalAirport": segment.get("arrival_airport", ""),
        "arrivalCode": segment.get("arrival_code", ""),
        "arrivalTime": segment.get("arrival_time", ""),
        "date": segment.get("date", ""),
    }


def convert_to_canonical_offer(result: ScrapeResult, offer_id: str = "") -> dict:
    """
    Convert Python ScrapeResult to TypeScript CanonicalOffer format.
    
    Returns a dict that matches the TypeScript CanonicalOffer interface.
    """
    result_dict = result.to_dict()
    
    # Determine offer type
    offer_type = "package"
    if result.flight.is_populated and not result.hotel.is_populated:
        offer_type = "flight"
    elif result.hotel.is_populated and not result.flight.is_populated:
        offer_type = "hotel"
    
    # Convert flight data
    flight_data = None
    if result.flight.is_populated:
        flight_dict = result_dict.get("flight", {})
        flight_data = {
            "outbound": convert_flight_segment(flight_dict.get("outbound", {})),
            "return": convert_flight_segment(flight_dict.get("return", {})),
        }
    
    # Convert hotel data
    hotel_data = None
    if result.hotel.is_populated:
        hotel_dict = result_dict.get("hotel", {})
        hotel_data = {
            "name": hotel_dict.get("name", ""),
            "slug": hotel_dict.get("slug"),
            "area": hotel_dict.get("area", ""),
            "areaType": hotel_dict.get("area_type", ""),
            "starRating": hotel_dict.get("star_rating"),
            "access": hotel_dict.get("access", []),
            "amenities": hotel_dict.get("amenities", []),
            "roomType": hotel_dict.get("room_type"),
        }
    
    # Convert date pricing from dict to array
    date_pricing_array = []
    if result.date_pricing:
        for date_key, dp in result.date_pricing.items():
            date_pricing_array.append({
                "date": date_key,
                "pricePerPerson": dp.price,
                "priceTotal": dp.price * 2 if dp.price else None,  # Assume 2 pax
                "availability": dp.availability,
                "notes": dp.notes or None,
            })
    
    # Find best value date
    best_value = None
    if date_pricing_array:
        valid_prices = [dp for dp in date_pricing_array if dp["pricePerPerson"]]
        if valid_prices:
            best = min(valid_prices, key=lambda x: x["pricePerPerson"])
            best_value = {
                "date": best["date"],
                "pricePerPerson": best["pricePerPerson"],
                "priceTotal": best["priceTotal"] or best["pricePerPerson"] * 2,
            }
    
    # Build canonical offer
    canonical = {
        "id": offer_id or f"{result.source_id}_{hash(result.url) & 0xFFFFFFFF:08x}",
        "sourceId": result.source_id,
        "type": offer_type,
        "title": result.title,
        "url": result.url,
        "currency": result.price.currency,
        "pricePerPerson": result.price.per_person or 0,
        "priceTotal": result.price.total,
        "availability": "available" if result.price.seats_available else "unknown",
        "scrapedAt": result.scraped_at,
    }
    
    # Add optional fields
    if result.baggage_included is not None:
        canonical["baggageIncluded"] = result.baggage_included
    if result.baggage_kg is not None:
        canonical["baggageKg"] = result.baggage_kg
    if flight_data:
        canonical["flight"] = flight_data
    if hotel_data:
        canonical["hotel"] = hotel_data
    if result.inclusions:
        canonical["includes"] = result.inclusions
    if date_pricing_array:
        canonical["datePricing"] = date_pricing_array
    if best_value:
        canonical["bestValue"] = best_value
    
    return canonical


def convert_scrape_result_file(input_path: str, output_path: str, offer_id: str = ""):
    """
    Convert a Python scrape result JSON file to TypeScript CanonicalOffer format.
    
    Args:
        input_path: Path to Python ScrapeResult JSON (from scrape_package.py)
        output_path: Path to write TypeScript CanonicalOffer JSON
        offer_id: Optional offer ID (auto-generated if not provided)
    """
    import json
    from .schema import ScrapeResult
    
    with open(input_path, encoding="utf-8") as f:
        data = json.load(f)
    
    result = ScrapeResult.from_dict(data)
    canonical = convert_to_canonical_offer(result, offer_id)
    
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(canonical, f, ensure_ascii=False, indent=2)
    
    print(f"Converted {input_path} → {output_path}")
    print(f"  Type: {canonical['type']}")
    print(f"  Price: {canonical['currency']} {canonical['pricePerPerson']}/person")
    if canonical.get("datePricing"):
        print(f"  Date pricing: {len(canonical['datePricing'])} dates")


if __name__ == "__main__":
    import sys
    
    if len(sys.argv) < 3:
        print("Usage: python -m scrapers.converter <input.json> <output.json> [offer_id]")
        sys.exit(1)
    
    input_file = sys.argv[1]
    output_file = sys.argv[2]
    offer_id = sys.argv[3] if len(sys.argv) > 3 else ""
    
    convert_scrape_result_file(input_file, output_file, offer_id)
