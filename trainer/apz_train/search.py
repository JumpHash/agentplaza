"""Hyperparameter search: grid, optuna (TPE) and population-based training (PBT).

The search space (job `search.space`): a list is a set of choices, {"low", "high", "log"?, "int"?} is a
range, and a scalar is fixed. Trials are sub-runs of Trainer sharing one GPU-hour budget; each trial
is scored on the validation split. The best trial's weights are returned for export. Search runs in a
single process (world 1); it is not resumable at the search level.
"""

import copy
import itertools
import math
import random

from .events import emit, log


def _split(space):
    choices = {k: v for k, v in space.items() if isinstance(v, list)}
    ranges = {k: v for k, v in space.items() if isinstance(v, dict)}
    fixed = {k: v for k, v in space.items() if not isinstance(v, (list, dict))}
    return choices, ranges, fixed


def _sample_range(spec, rng):
    low, high = float(spec["low"]), float(spec["high"])
    if spec.get("log"):
        x = math.exp(rng.uniform(math.log(low), math.log(high)))
    else:
        x = rng.uniform(low, high)
    return int(round(x)) if spec.get("int") else x


def _sample(space, rng):
    choices, ranges, fixed = _split(space)
    out = dict(fixed)
    out.update({k: rng.choice(v) for k, v in choices.items()})
    out.update({k: _sample_range(v, rng) for k, v in ranges.items()})
    return out


def _perturb(params, space, rng):
    _, ranges, _ = _split(space)
    out = dict(params)
    for k, spec in ranges.items():
        x = out[k] * rng.choice([0.8, 1.2])
        x = min(max(x, float(spec["low"])), float(spec["high"]))
        out[k] = int(round(x)) if spec.get("int") else x
    return out


def _grid(space, trials):
    choices, _, fixed = _split(space)
    keys = list(choices)
    for combo in itertools.islice(itertools.product(*[choices[k] for k in keys]), trials):
        yield {**fixed, **dict(zip(keys, combo))}


def _record(best, params, trainer, metrics, trial):
    obj = metrics["expected_cost"]
    emit("trial", trial=trial, params=params, objective=obj, metrics=metrics,
         gpu_hours=trainer.budget.hours())
    if best.get("objective") is None or obj < best["objective"]:
        best.update(objective=obj, params=params, metrics=metrics, trial=trial,
                    state=trainer.full_state())


def run_grid(make, space, trials, val_ds, best):
    for k, params in enumerate(_grid(space, trials)):
        t = make(params, f"trial-{k}")
        t.fit(until=int(t.cfg["steps"]))
        _record(best, params, t, t.evaluate(val_ds), k)
        if t.budget.exhausted():
            break
    return best


def run_optuna(make, space, trials, val_ds, budget, seed, best):
    try:
        import optuna
    except ImportError as e:
        raise RuntimeError("search.method optuna needs the 'search' extra (apz-train[search])") from e
    choices, ranges, fixed = _split(space)

    def objective(trial):
        if budget.exhausted():
            trial.study.stop()
            raise optuna.TrialPruned()
        params = dict(fixed)
        params.update({k: trial.suggest_categorical(k, v) for k, v in choices.items()})
        for k, spec in ranges.items():
            if spec.get("int"):
                params[k] = trial.suggest_int(k, int(spec["low"]), int(spec["high"]), log=bool(spec.get("log")))
            else:
                params[k] = trial.suggest_float(k, float(spec["low"]), float(spec["high"]), log=bool(spec.get("log")))
        t = make(params, f"trial-{trial.number}")
        t.fit(until=int(t.cfg["steps"]))
        metrics = t.evaluate(val_ds)
        _record(best, params, t, metrics, trial.number)
        return metrics["expected_cost"]

    study = optuna.create_study(direction="minimize", sampler=optuna.samplers.TPESampler(seed=seed))
    study.optimize(objective, n_trials=trials)
    return best


def run_pbt(make, space, population, generations, steps, val_ds, budget, seed, best):
    """Members train in turns, one interval per generation. The bottom quartile copies the weights and
    optimizer state of a top-quartile member with the same architecture (list-valued params), then
    perturbs its continuous hyperparameters."""
    rng = random.Random(seed)
    choices, _, _ = _split(space)
    members = []
    for i in range(population):
        params = _sample(space, rng)
        members.append({"params": params, "t": make(params, f"pbt-{i}"), "obj": None})
    interval = max(1, steps // generations)
    for g in range(generations):
        for i, m in enumerate(members):
            m["t"].fit(until=min(steps, (g + 1) * interval))
            m["metrics"] = m["t"].evaluate(val_ds)
            m["obj"] = m["metrics"]["expected_cost"]
            emit("trial", trial=i, generation=g, params=m["params"], objective=m["obj"],
                 metrics=m["metrics"], gpu_hours=budget.hours())
        if g == generations - 1 or budget.exhausted():
            break
        ranked = sorted(members, key=lambda m: m["obj"])
        q = max(1, population // 4)
        top = ranked[:q]
        for loser in ranked[-q:]:
            same_arch = [t for t in top if all(t["params"][k] == loser["params"][k] for k in choices)]
            if not same_arch or loser in top:
                continue
            src = rng.choice(same_arch)
            loser["t"].set_state(copy.deepcopy(src["t"].get_state()))
            loser["params"] = _perturb(src["params"], space, rng)
            loser["t"].cfg.update(loser["params"])
            log(f"pbt: member copied a top member at generation {g}")
    winner = min(members, key=lambda m: m["obj"])
    _record(best, winner["params"], winner["t"], winner["metrics"], "pbt-best")
    return best


def run(method, make, space, trials, steps, val_ds, budget, seed):
    """Returns the best trial: {objective, params, metrics, trial, state}."""
    best = {"objective": None}
    if method == "grid":
        run_grid(make, space, trials, val_ds, best)
    elif method == "optuna":
        run_optuna(make, space, trials, val_ds, budget, seed, best)
    elif method == "pbt":
        run_pbt(make, space, trials, 3, steps, val_ds, budget, seed, best)
    else:
        raise ValueError(f"unknown search method {method!r}")
    if best.get("state") is None:
        raise RuntimeError("search produced no trial (budget exhausted before the first one?)")
    return best
