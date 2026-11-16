"""Opening and phase sensitivity of normalized useful economic output.

This is not a battle simulator. All numbers are hypothetical.
Run: python3 phase_model.py --output phase-results.json
Costs/output already include resource-conversion opportunity costs.
The common seed flow represents a proposed bootstrap, not implemented income.
"""
import argparse
import json
import math
from dataclasses import dataclass


@dataclass(frozen=True)
class Case:
    price: float = 300.0
    increment: float = 5.0
    build_time: float = 10.0
    normalized_field_budget: float = math.inf


def capacity(t):
    """Exogenous equal territory access, not an expansion AI or free game rule."""
    if t < 300:
        return 20.0
    if t < 900:
        return 60.0
    if t < 1800:
        return 120.0
    return 200.0


def simulate(case, seconds=3600, dt=0.1):
    seed = 10.0
    investment_fraction = 0.30
    economy_cash, army_cash = 600.0, 400.0
    installed = 0.0
    finish = None
    packages = 0
    first = exhausted_at = None
    budget_used = invested = produced = 0.0
    checkpoints = {}
    for tick in range(round(seconds / dt)):
        t = tick * dt
        if finish is not None and t >= finish - 1e-7:
            installed += case.increment
            packages += 1
            first = t if first is None else first
            finish = None
        room = capacity(t)
        remaining = case.normalized_field_budget - budget_used
        admitted = installed + case.increment <= room + 1e-7 and remaining > 1e-7
        if finish is None and admitted and economy_cash >= case.price - 1e-7:
            economy_cash -= case.price
            invested += case.price
            finish = t + case.build_time
        economic_flow = installed
        if math.isfinite(case.normalized_field_budget):
            economic_flow = min(economic_flow, max(0.0, remaining) / dt)
        budget_used += economic_flow * dt
        if math.isfinite(case.normalized_field_budget) and budget_used >= case.normalized_field_budget - 1e-7 and exhausted_at is None:
            exhausted_at = t + dt
        income = (seed + economic_flow) * dt
        produced += income
        reserved_increment = case.increment if finish is not None else 0.0
        can_grow_further = installed + reserved_increment < room - 1e-7 and remaining > 1e-7
        earmark = investment_fraction * income if (finish is not None or can_grow_further) else 0.0
        economy_cash += earmark
        army_cash += income - earmark
        at = round(t + dt, 6)
        if at in (60, 180, 300, 600, 900, 1200, 1800, 3600):
            checkpoints[str(int(at))] = {
                "installed_useful_capacity": round(installed, 6),
                "current_useful_income": round(seed + economic_flow, 6),
                "army_budget_under_fixed_policy": round(army_cash, 6),
                "unspent_economy_cash": round(economy_cash, 6),
                "economy_paid": round(invested, 6),
                "cumulative_useful_output": round(produced, 6),
                "normalized_field_budget_used": round(budget_used, 6),
            }
    assert math.isclose(army_cash + economy_cash + invested, 1000.0 + produced, abs_tol=1e-5)
    if math.isfinite(case.normalized_field_budget):
        assert budget_used <= case.normalized_field_budget + 1e-6
    return {"first_package_active_seconds": first,
            "resource_exhaustion_seconds": exhausted_at,
            "checkpoints": checkpoints}


def results():
    small = simulate(Case())
    matched = simulate(Case())
    large = simulate(Case(price=1200.0, increment=20.0, build_time=40.0))
    cheaper = simulate(Case(price=294.0, build_time=9.8))
    finite_a = simulate(Case(normalized_field_budget=60000))
    finite_b = simulate(Case(normalized_field_budget=60000))
    assert small == matched
    assert finite_a == finite_b
    assert math.isclose(300 / 5, 1200 / 20)
    assert large["first_package_active_seconds"] > small["first_package_active_seconds"]
    assert small["checkpoints"]["60"]["current_useful_income"] > large["checkpoints"]["60"]["current_useful_income"]
    assert small["checkpoints"]["1800"]["current_useful_income"] > finite_a["checkpoints"]["1800"]["current_useful_income"]
    r = math.log(2) / 240
    continuous = {str(t): math.exp(0.02 * r * t) for t in (300, 1200, 1800)}
    return {
        "status": "Illustrative design model, not measured faction balance",
        "assumptions": {
            "initial_matter": 1000, "initial_economy_allocation": 600,
            "initial_army_allocation": 400, "common_seed_useful_output": 10,
            "fixed_investment_fraction": 0.30, "dt_seconds": 0.1,
            "one_construction_slot": True,
            "territory_useful_capacity_schedule": [[0,20],[300,60],[900,120],[1800,200]],
            "costs_and_rates_already_net_of_energy_conversion": True,
            "territory_unlocked_exogenously_not_a_free_game_mechanic": True,
            "seed_is_a_hypothetical_bootstrap_not_existing_game_stats": True,
            "no_combat_reclaim_transport_or_optimal_strategy_search": True,
            "army_budget_is_fixed_policy_allocation_not_fighting_strength": True,
        },
        "cases": {
            "small_packages": small,
            "matched_third_race": matched,
            "large_packages_same_payback": large,
            "two_percent_cheaper_packages": cheaper,
            "finite_budget_60000": finite_a,
            "matched_finite_third": finite_b,
        },
        "continuous_growth_sensitivity": {
            "note": "Unbounded continuous approximation, not the staged-territory simulation",
            "reference_doubling_seconds": 240,
            "fractional_growth_rate_advantage": 0.02,
            "income_ratio_at_seconds": continuous,
        },
        "validation": "Budget conservation, identical matched cases, equal-payback packet-size mismatch and finite-source symmetry passed",
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
