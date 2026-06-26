"""SemaStateMixin: pre-walk semantic fact population.

Move-only extraction from frontend/__init__.py. This lowering authority owns the
single pre-walk bridge from immutable ``SemaResult`` facts into the generator's
walk-time module state.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING

from dataclasses import replace

from molt.frontend.sema import (
    SemaResult,
    analyze_module,
    class_facts_with_super_fold_sound_methods,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class SemaStateMixin(_MixinBase):
    def _module_stable_funcs(self, node: ast.Module) -> set[str]:
        counts, funcs, dynamic = self._collect_module_assignments(node)
        if dynamic:
            return set()
        global_rebinds = self._collect_global_rebinds(node)
        return {
            name
            for name in funcs
            if counts.get(name, 0) == 1 and name not in global_rebinds
        }

    def _populate_sema_state(self, node: ast.Module) -> SemaResult:
        """Compute the module's :class:`SemaResult` once, pre-walk, and populate
        the existing god-object pre-walk state dicts from it (doc 44 §F2b).

        This is the additive shim: the lowering walk continues to read the same
        ``self.module_*`` dicts, so the emitted IR is byte-identical to the prior
        inline-computation path.  Each dict is filled from exactly one
        ``SemaResult`` field — this assignment table IS the F2c worklist (the
        read-sites that F2c rewires onto ``self._sema`` and then deletes the dict).

        Shim inventory — god-object dict  ←  SemaResult source:

          self.module_const_dicts         ← sema.const_dicts
                                            (sema/constenv.collect_module_const_dicts)
          self.module_declared_funcs      ← sema.function_meta.declared_funcs
                                            (sema/funcmeta.collect_module_func_kinds)
          self.module_declared_classes    ← sema.function_meta.declared_classes
                                            (sema/funcmeta.collect_module_class_names)
          sema.class_graph                → read directly by classgraph fold
                                            queries (no god-object dict shim)
          sema.class_facts                → read directly by classgraph fold
                                            queries (no god-object dict shim)
          self.module_func_defaults       ← known_func_defaults override, else
                                            sema.function_meta.defaults
                                            (sema/funcmeta.collect_module_func_defaults)

        These four dicts are each written exactly once (here) and only *read* during
        the walk — verified against HEAD: no ``.add``/``.pop``/``[k]=`` mutation of
        any of them occurs in the visit/emit methods.  Walk-mutated cursors
        (``const_ints`` written in ``emit()``; ``exact_locals`` mutated across
        visit methods; the per-function scope dicts populated lazily in
        ``start_function``) are deliberately NOT Sema facts and stay where they are
        (doc 44 risk #1: mis-classifying a cursor as a fact is a miscompile).
        """
        sema = analyze_module(node)
        sema = replace(
            sema,
            class_facts=class_facts_with_super_fold_sound_methods(
                class_graph=sema.class_graph,
                class_facts=sema.class_facts,
                imported_classes=self.known_classes,
                module_name=self.module_name,
                entry_module=self.entry_module,
            ),
        )
        self._sema = sema
        self.module_const_dicts = sema.const_dicts
        self.module_declared_funcs = sema.function_meta.declared_funcs
        self.module_declared_classes = sema.function_meta.declared_classes
        self.module_func_defaults = self.known_func_defaults.get(
            self.module_name, sema.function_meta.defaults
        )
        return sema
