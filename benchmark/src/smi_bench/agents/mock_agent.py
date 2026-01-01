
from __future__ import annotations

import random
from dataclasses import dataclass


@dataclass
class MockAgent:
    behavior: str
    seed: int = 0

    def predict_key_types(self, *, truth_key_types: set[str]) -> set[str]:
        """
        Mock agent for target discovery.

        behaviors:
          - perfect: returns the truth set
          - empty: returns empty set
          - random: deterministic random subset of truth
          - noisy: random subset of truth + random junk strings
        """
        if self.behavior == "perfect":
            return set(truth_key_types)
        if self.behavior == "empty":
            return set()

        rng = random.Random(self.seed)

        if self.behavior == "random":
            out = set()
            for t in truth_key_types:
                if rng.random() < 0.5:
                    out.add(t)
            return out

        if self.behavior == "noisy":
            out = set()
            for t in truth_key_types:
                if rng.random() < 0.7:
                    out.add(t)
            for _ in range(5):
                out.add(f"0xdead::{rng.randint(0, 9999)}::Fake")
            return out

        raise ValueError(f"unknown mock behavior: {self.behavior}")
