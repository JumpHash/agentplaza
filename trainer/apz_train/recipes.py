"""Recipe loader. A recipe is a directory `<recipes>/<name with _>/` holding recipe.json (id, schema,
defaults) and recipe.py (build_model, dataset, loss, metrics, example_input). Recipes come from the
owner-signed foundry-recipes checkout, mounted read-only; the bundled ones are for tests and CI."""

import importlib.util
import json
import os
import re

BUNDLED = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "recipes")
_ID = re.compile(r"[a-z0-9-]+@[0-9]+")


class Recipe:
    def __init__(self, meta, module):
        self.meta, self.id, self.mod = meta, meta["id"], module
        self.inputs = meta["input"]["names"]
        self.outputs = meta["output"]["names"]
        self.defaults = dict(meta["defaults"])
        self.shard = bool(meta.get("shard", False))

    def bind(self, cfg):
        """Recipes that keep loss/metric settings in module state re-bind them from the config here."""
        bind = getattr(self.mod, "bind", None)
        if bind is not None:
            bind(cfg)

    def build_model(self, cfg):
        return self.mod.build_model(cfg)

    def dataset(self, paths, split, cfg):
        return self.mod.dataset(paths, split, cfg)

    def loss(self, batch, out):
        return self.mod.loss(batch, out)

    def metrics(self, batch, out):
        return self.mod.metrics(batch, out)

    def example_input(self):
        return self.mod.example_input()


def load(recipe_id, recipes_dir=None):
    if not _ID.fullmatch(recipe_id or ""):
        raise ValueError(f"bad recipe id {recipe_id!r}")
    name, ver = recipe_id.split("@")
    base = recipes_dir or os.environ.get("APZ_RECIPES") or BUNDLED
    folder = os.path.join(base, name.replace("-", "_"))
    with open(os.path.join(folder, "recipe.json"), encoding="utf-8") as f:
        meta = json.load(f)
    if meta["id"] != recipe_id:
        raise ValueError(f"{folder} declares {meta['id']!r}, job asks for {recipe_id!r}")
    spec = importlib.util.spec_from_file_location(f"apz_recipe_{name.replace('-', '_')}_{ver}",
                                                  os.path.join(folder, "recipe.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return Recipe(meta, module)
