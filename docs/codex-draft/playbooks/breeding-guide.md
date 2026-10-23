# Breeding guide
How to train a small Foundry model that replaces expensive LLM calls, starting with the rung predictor, without fooling yourself on the numbers. Breeders use it; the owner uses it to judge a model before promotion.

## When to use
- You are preparing data and a job spec for a vetted recipe, such as `rung-predictor@1`.
- Results came back and you must decide whether to iterate, stop or request shadow mode.
- A job looks like it needs more than 8 GPU-hours, or a method needs a recipe that does not exist.

## Steps
1. Read the recipe and the current Foundry state with `read_foundry`: existing models, their cards and shadow-mode results. Know the baseline you must beat. For the rung predictor that is the plain cascade.
2. Export data with `read_cascade_data` (`since`, optional `roles`). Labels are cascade outcomes: task features mapped to the lowest rung that passed verification.
3. Check the data in your sandbox with `bash`: label balance, time range, duplicates and missing features. Use real data first; synthetic data only to fill gaps, and never for validation.
4. Remove tainted traces. Tainted data never enters training.
5. Exclude the frozen, human-curated validation set of real tasks from training. It must stay frozen; never tune on it repeatedly or add to it from your own exports. Use a time-based split as well: train on older tasks, test on newer ones.
6. Hunt for leakage: features that encode the label (for example a field recorded after the rung was known), near-duplicate tasks across splits, or validation tasks that also appear in training.
7. Write the job spec with the **training-job** schema: `job_id`, `recipe`, `datasets`, `search` (`method`: `optuna`, `pbt` or `grid`; `trials`; `space`), `budget_gpu_hours`, `target` and `export` (`onnx`). Only vetted recipes from the owner-signed `foundry-recipes` repo run; you never run arbitrary code on the GPU host.
8. Keep `budget_gpu_hours` at 8 or less. A larger job needs owner approval. Never split one search into several small jobs to stay under the limit.
9. Submit with `submit_training_job`. GPU time is metered against the job, separate from your own model-call budget.
10. Read the results and the model card: data provenance, metrics, calibration and size. Judge on the recipe's objective and expected cost including escalation cost, not plain accuracy.
11. If it beats the current path on the frozen set at equal or better pass rate, request shadow mode. The model runs beside the plain cascade; the owner promotes it only if it lowers cost at an equal or better pass rate. Otherwise iterate or stop.
12. If results look too good, suspect leakage and escalate if you cannot explain it. If a promising method needs a new recipe, propose it as a branch of the recipe repo for the owner to ratify.

## Checklist
- [ ] The data range, label balance and feature list are recorded.
- [ ] No tainted trace is in any training dataset.
- [ ] The frozen validation set is excluded from training and unchanged.
- [ ] Synthetic data, if any, is only in training, never in validation.
- [ ] A time-based split was used alongside the frozen set.
- [ ] Features were checked for label leakage and cross-split duplicates.
- [ ] The recipe is a vetted one from `foundry-recipes`.
- [ ] `budget_gpu_hours` is at most 8, or the owner has approved a larger job.
- [ ] The search was not split into several jobs to dodge the limit.
- [ ] The decision uses expected cost and calibration, not accuracy alone.
- [ ] Shadow mode is requested only after beating the baseline on the frozen set.
- [ ] Export is ONNX; weights are never pickled.

## Pitfalls
- Requesting shadow mode after 99.8% validation accuracy on a first run skips the obvious leakage check, so suspect leakage, check the splits and escalate if unresolved.
- Training on tainted traces lets an injected task shape the model that routes everyone's work, so filter on taint before anything else.
- Tuning hyperparameters against the frozen set turns it into training data, so tune on the time-based split and touch the frozen set only for final evaluation.
- Running five small jobs instead of one large one to stay under 8 GPU-hours is threshold splitting, so ask the owner for approval for the real size.
- Using synthetic tasks for validation hides contamination and collapse, so validate only on real tasks.
- Promoting on accuracy alone can raise cost if the model is badly calibrated, so report expected cost and calibration with every result.
- Trying a new architecture by bending a vetted recipe's parameters beyond its intent sidesteps review, so propose a recipe change for the owner instead.
- Reporting savings before shadow mode measures them is a misleading claim, so let the owner judge savings from shadow-mode data.
