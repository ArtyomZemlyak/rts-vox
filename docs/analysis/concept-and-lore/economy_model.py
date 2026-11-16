"""Illustrative faction-economy model, not a game simulation or balance preset.

Run: python3 economy_model.py --output economy-results.json
All values are arbitrary normalized design units, not extracted game stats.
Energy is a flow; matter can be stored. Common commander income is excluded.
"""

import argparse
import json
import math
import random
from dataclasses import dataclass


def mining_output(raw_matter, energy_per_burned_matter=10.0,
                  build_energy_per_matter=2.0, other_power=0.0,
                  reactor_power_cap=math.inf):
    """Maximum sustained useful matter spend, including its building power."""
    a, e = energy_per_burned_matter, build_energy_per_matter
    return max(0.0, min((a * raw_matter - other_power) / (a + e),
                        (reactor_power_cap - other_power) / e))


def synthesis_output(external_power, energy_per_fabricated_matter=10.0,
                     build_energy_per_matter=2.0, other_power=0.0,
                     fabricator_matter_cap=math.inf):
    k, e = energy_per_fabricated_matter, build_energy_per_matter
    return max(0.0, min((external_power - other_power) / (k + e),
                        fabricator_matter_cap))


def reserve_duration(primary_fraction, side, reserve=600.0,
                     target_spend=100.0, raw_matter=120.0,
                     external_power=1200.0, a=10.0, k=10.0, e=2.0):
    """Time to maintain target spending after source damage, other_power=0.

    A can burn stored matter; B cannot burn it in this faction proposal.
    B reserves bypass fabrication but cannot supply missing operational power.
    Converter capacities assumed sufficient; already stored energy excluded.
    """
    operational_power = e * target_spend
    if side == "mining":
        total_matter_needed = target_spend + operational_power / a
        deficit = total_matter_needed - raw_matter * primary_fraction
    else:
        power = external_power * primary_fraction
        if power < operational_power - 1e-9:
            return 0.0
        actual_fabrication = (power - operational_power) / k
        deficit = target_spend - actual_fabrication
    return None if deficit <= 1e-9 else reserve / deficit


@dataclass(frozen=True)
class GrowthCase:
    side: str
    max_packages: int = 1000000
    accessible_ore: float = math.inf


def grow(case, seconds=1200, dt=0.1, reinvest=0.30, cost=1200.0,
         min_build_seconds=30.0):
    """Six initial complete economy packages, one build slot, no cash reserve.

    Each paid package adds 20 M/s gross mining OR 200 E/s external input,
    plus enough converter capacity. Its total capital cost is 1200 M.
    All useful spending also pays 2 E/M; counted in the output formulas.
    Investment and army spending share that useful-spending budget.
    Completed packages require payment AND minimum construction duration.
    Ore input is physically exhausted at accessible_ore for mining only.
    No combat, reclaim, transport, startup commander, refunds, or unit AI.
    'Army' is cumulative spend available for units, not combat strength.
    """
    count, paid, start = 6, 0.0, None
    ore_used = army = invested = total_spend = 0.0
    checkpoints = {}
    exhausted_at = None
    for tick in range(round(seconds / dt)):
        t = tick * dt
        if case.side == "mining":
            raw = 20.0 * count
            if math.isfinite(case.accessible_ore):
                raw = min(raw, max(0.0, case.accessible_ore - ore_used) / dt)
            ore_used += raw * dt
            if math.isfinite(case.accessible_ore) and ore_used >= case.accessible_ore - 1e-6 and exhausted_at is None:
                exhausted_at = t + dt
            output = mining_output(raw)
        else:
            output = synthesis_output(200.0 * count)
        if start is None and count < case.max_packages:
            start = t
        payment = 0.0
        if start is not None:
            payment = min(reinvest * output * dt, max(0.0, cost - paid))
            paid += payment
        invested += payment
        army += output * dt - payment
        total_spend += output * dt
        if start is not None and paid >= cost - 1e-7 and t + dt - start >= min_build_seconds - 1e-7:
            count += 1
            start, paid = None, 0.0
        at = round(t + dt, 6)
        if at in (300, 600, 1200):
            checkpoints[str(int(at))] = {
                "packages": count, "army_matter_spend": round(army, 6),
                "economy_matter_spend": round(invested, 6),
                "ore_used": round(ore_used, 6),
            }
    assert math.isclose(army + invested, total_spend, rel_tol=1e-10)
    assert math.isclose(invested, (count - 6) * cost + paid, abs_tol=1e-5)
    if math.isfinite(case.accessible_ore):
        assert ore_used <= case.accessible_ore + 1e-5
    return {"checkpoints": checkpoints,
            "exhausted_at_seconds": None if exhausted_at is None else round(exhausted_at, 6)}


def validate():
    assert math.isclose(mining_output(120), 100)
    assert math.isclose(synthesis_output(1200), 100)
    assert math.isclose(mining_output(120, other_power=300), 75)
    assert math.isclose(synthesis_output(1200, other_power=300), 75)
    rng = random.Random(20261116)
    for _ in range(1000):
        x = rng.uniform(0.0, 1000.0)
        a = rng.uniform(0.1, 100.0)
        e = rng.uniform(0.01, 10.0)
        d = rng.uniform(0.0, a * x + 1.0)
        ma = mining_output(x, a, e, d)
        mb = synthesis_output(a * x, a, e, d)
        assert math.isclose(ma, mb, abs_tol=1e-9)
        if ma > 0:
            burned = (d + e * ma) / a
            assert math.isclose(ma + burned, x, rel_tol=1e-9)
        if mb > 0:
            assert math.isclose(a * mb + e * mb + d, a * x, rel_tol=1e-9)
    assert math.isclose(reserve_duration(0, "mining"), 5)
    assert reserve_duration(0, "synthesis") == 0
    assert math.isclose(reserve_duration(0.5, "mining"), 10)
    assert math.isclose(reserve_duration(0.5, "synthesis"), 10)
    mirrored_a = grow(GrowthCase("mining"))
    mirrored_b = grow(GrowthCase("synthesis"))
    for t in mirrored_a["checkpoints"]:
        ra, rb = mirrored_a["checkpoints"][t], mirrored_b["checkpoints"][t]
        assert ra["packages"] == rb["packages"]
        assert math.isclose(ra["army_matter_spend"], rb["army_matter_spend"], abs_tol=1e-6)
    return mirrored_a, mirrored_b


def results():
    mirrored_a, mirrored_b = validate()
    alpha = 0.25
    n = 6
    clustered = 200 * n / (1 + alpha * (n - 1))
    return {
        "status": "Illustrative model; NOT measured game balance",
        "units": {"matter": "M", "energy_flow": "E/s", "time": "seconds"},
        "common_commander_income": "Excluded; energy charging not selected in game design",
        "baseline": {"mining_gross_matter_per_second": 120,
                     "external_power": 1200, "burn_energy_per_matter": 10,
                     "fabrication_energy_per_matter": 10,
                     "build_energy_per_matter": 2, "other_power": 0,
                     "mining_useful_matter_per_second": mining_output(120),
                     "synthesis_useful_matter_per_second": synthesis_output(1200)},
        "flow_scenarios": [
            {"name": "baseline", "mining": mining_output(120), "synthesis": synthesis_output(1200)},
            {"name": "half_primary_source", "mining": mining_output(60), "synthesis": synthesis_output(600)},
            {"name": "additional_300_power_for_other_uses", "mining": mining_output(120, other_power=300), "synthesis": synthesis_output(1200, other_power=300)},
        ],
        "reserve_seconds_with_600_matter": {
            "half_source": {"mining": reserve_duration(0.5, "mining"), "synthesis": reserve_duration(0.5, "synthesis")},
            "complete_source_loss": {"mining": reserve_duration(0, "mining"), "synthesis": reserve_duration(0, "synthesis")},
            "note": "Sufficient converter capacity; maintain 100 M/s plus 200 E/s work; B cannot burn stored matter"},
        "round_trip": {"burn_10_fabrication_10_return_fraction": 1.0,
                       "burn_10_fabrication_12_return_fraction": 10/12,
                       "burn_12_fabrication_10_return_fraction": 1.2,
                       "synthesis_power_for_matching_100_matter_at_cost_12": 1400,
                       "synthesis_output_at_cost_12_with_300_other_power": synthesis_output(1400, 12, other_power=300)},
        "growth_assumptions": {"dt": 0.1, "initial_complete_packages": 6,
                               "cost_per_package_matter": 1200, "min_build_seconds": 30,
                               "build_slots": 1, "investment_fraction": 0.30,
                               "gross_mining_per_package": 20, "external_power_per_package": 200,
                               "no_reserves_no_combat_no_reclaim_no_transport": True},
        "growth": {"mining_unbounded": mirrored_a, "synthesis_unbounded": mirrored_b,
                   "mining_local_ore_72000": grow(GrowthCase("mining", accessible_ore=72000)),
                   "synthesis_local_cap_8_packages": grow(GrowthCase("synthesis", max_packages=8))},
        "spatial_saturation_example": {"formula": "P(n)=200*n/(1+0.25*(n-1))",
                                      "six_in_independent_areas_power": 1200,
                                      "six_in_one_area_power": clustered,
                                      "six_in_one_area_useful_matter": synthesis_output(clustered),
                                      "note": "A proposed map constraint, not an adopted rule or calibrated parameter"},
        "validation": "1000 mirrored-flow cases, budget identities, reserve cases and delayed-growth equivalence passed",
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
