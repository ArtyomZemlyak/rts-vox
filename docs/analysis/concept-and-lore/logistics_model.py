"""Illustrative steady-state logistics costs; not game-unit statistics.

Run: python3 logistics_model.py --output logistics-results.json
Three independent carrier pools: mine->store, store->fuel, store->building.
Energy is a flow; no combat or autonomous remote-construction rule modeled.
"""
import argparse
import json
import math


def haul_rate(count, payload, distance, speed, handling=10.0):
    return count * payload / (2 * distance / speed + handling)


def required_carriers(flow, payload, distance, speed, handling=10.0):
    return math.ceil(flow * (2 * distance / speed + handling) / payload - 1e-10)


def useful_output(mining, mine_haul, fuel_haul, build_haul,
                  burn_efficiency=10.0, build_energy=2.0, other_power=0.0):
    delivered = min(mining, mine_haul)
    matter_bound = (burn_efficiency * delivered - other_power) / (burn_efficiency + build_energy)
    fuel_bound = (burn_efficiency * fuel_haul - other_power) / build_energy
    return max(0.0, min(matter_bound, fuel_bound, build_haul))


def results():
    raw, a, e, target = 120.0, 10.0, 2.0, 100.0
    payload, speed, handling, carrier_price = 300.0, 10.0, 10.0, 80.0
    fixed_fleet = 12
    internal_distance = 50.0
    fuel = e * target / a
    fuel_count = required_carriers(fuel, payload, internal_distance, speed, handling)
    building_count = required_carriers(target, payload, internal_distance, speed, handling)
    fuel_cap = haul_rate(fuel_count, payload, internal_distance, speed, handling)
    build_cap = haul_rate(building_count, payload, internal_distance, speed, handling)
    core_price = 2400.0
    third_output = 1200.0 / (10.0 + e)
    rows = []
    for distance in (50.0, 100.0, 200.0, 300.0):
        incoming = haul_rate(fixed_fleet, payload, distance, speed, handling)
        output = useful_output(raw, incoming, fuel_cap, build_cap, a, e)
        needed = required_carriers(raw, payload, distance, speed, handling)
        capex = core_price + carrier_price * (needed + fuel_count + building_count)
        transit = raw * distance / speed + fuel * internal_distance / speed + target * internal_distance / speed
        rows.append({
            "mine_store_distance_m": distance,
            "round_trip_seconds": 2 * distance / speed + handling,
            "mine_haul_capacity_with_12_carriers": incoming,
            "useful_output_with_12_mine_carriers": output,
            "mine_carriers_for_100_useful_output": needed,
            "all_carriers_for_100_useful_output": needed + fuel_count + building_count,
            "total_fixed_capital_for_100_useful_output": capex,
            "average_loaded_cargo_in_travel_at_target_output": transit,
            "third_same_core_capex_efficiency_ratio": capex / core_price,
            "third_if_priced_4200_capex_efficiency_ratio": capex / 4200.0,
            "third_price_band_for_within_5_percent_capex_efficiency": [capex / 1.05, capex / 0.95],
        })
        matched = useful_output(raw, haul_rate(needed, payload, distance, speed, handling), fuel_cap, build_cap, a, e)
        assert math.isclose(matched, target)
        assert incoming > 0 and output <= raw
    assert math.isclose(third_output, target)
    assert rows[0]["useful_output_with_12_mine_carriers"] >= rows[-1]["useful_output_with_12_mine_carriers"]
    assert math.isclose(useful_output(raw, 120, 10, 100), 50)
    assert math.isclose(useful_output(raw, 120, 30, 40), 40)
    near, far = rows[1], rows[3]
    bands_overlap = max(near["third_price_band_for_within_5_percent_capex_efficiency"][0], far["third_price_band_for_within_5_percent_capex_efficiency"][0]) <= min(near["third_price_band_for_within_5_percent_capex_efficiency"][1], far["third_price_band_for_within_5_percent_capex_efficiency"][1])
    assert not bands_overlap
    return {
        "status": "Hypothetical throughput model, not measured faction balance",
        "assumptions": {
            "gross_mining_matter_per_second": raw, "energy_per_burned_matter": a,
            "construction_energy_per_matter": e, "other_power": 0,
            "carrier_payload_matter": payload, "carrier_speed_m_per_second": speed,
            "total_handling_seconds_per_round_trip": handling,
            "carrier_price_matter": carrier_price,
            "productive_core_price_both_sides": core_price,
            "third_gross_external_power": 1200,
            "third_energy_per_fabricated_matter": 10,
            "third_useful_local_output": third_output,
            "fuel_and_construction_one_way_distance": internal_distance,
            "independent_fuel_carriers": fuel_count,
            "independent_construction_carriers": building_count,
            "internal_routes_not_shared_with_inbound_route": True,
            "no_refining_loss_attrition_traffic_congestion_or_startup_transient": True,
            "recoverable_cargo_inventory_not_added_to_equipment_capex": True,
            "same_hypothetical_carrier_for_all_legs_not_existing_game_roster": True,
        },
        "distance_scenarios": rows,
        "single_third_price_within_5_percent_for_100_and_300_m_exists": bands_overlap,
        "validation": "Flow limits, independent fuel/build bottlenecks, integer fleet sizing and non-overlapping equal-efficiency price bands passed",
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output")
    args = parser.parse_args()
    payload = json.dumps(results(), ensure_ascii=False, indent=2, allow_nan=False)
    if args.output:
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(payload + "\n")
    print(payload)
