// A seeded generator for property tests (the app has no property-testing dependency). Set LK_PROP_SEED to replay a
// failure; the seed is printed in the failure message.

/** mulberry32: small, fast, good enough to make test inputs. */
export function makeRng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export type Rng = {
  next(): number;
  int(maxExclusive: number): number;
  pick<T>(items: readonly T[]): T;
  string(alphabet: string, maxLength: number): string;
  bool(): boolean;
};

export function rngFrom(seed: number): Rng {
  const next = makeRng(seed);
  const int = (maxExclusive: number): number => Math.floor(next() * maxExclusive);
  return {
    next,
    int,
    pick: <T>(items: readonly T[]): T => {
      const item = items[int(items.length)];
      if (item === undefined) throw new Error("pick from an empty list");
      return item;
    },
    string: (alphabet, maxLength) => {
      const chars = Array.from(alphabet);
      const n = int(maxLength + 1);
      let out = "";
      for (let i = 0; i < n; i++) out += chars[int(chars.length)] ?? "";
      return out;
    },
    bool: () => next() < 0.5,
  };
}

export function propSeed(): number {
  const fromEnv = typeof process === "undefined" ? undefined : process.env["LK_PROP_SEED"];
  const parsed = fromEnv === undefined ? Number.NaN : Number(fromEnv);
  return Number.isFinite(parsed) ? parsed : 0x1ea5e7;
}

/**
 * Run `check` on `cases` generated inputs. On the first failure the error names the case, the seed and the input, so the
 * run can be replayed with LK_PROP_SEED.
 */
export function forAll<T>(cases: number, generate: (rng: Rng) => T, check: (input: T, index: number) => void, seed = propSeed()): void {
  const rng = rngFrom(seed);
  for (let i = 0; i < cases; i++) {
    const input = generate(rng);
    try {
      check(input, i);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`property failed at case ${i}, seed ${seed}, input ${JSON.stringify(input)}: ${message}`, { cause: error });
    }
  }
}
