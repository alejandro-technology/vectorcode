import { Task } from './types.js';

/**
 * Seeded PRNG using the mulberry32 algorithm.
 * Deterministic given the same seed — essential for reproducibility.
 */
function mulberry32(seed: number): () => number {
  return function () {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Convert a string seed into a numeric seed via simple hash.
 */
function hashSeed(str: string): number {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    const ch = str.charCodeAt(i);
    hash = ((hash << 5) - hash + ch) | 0;
  }
  return hash;
}

/**
 * Fisher–Yates shuffle using a seeded PRNG.
 */
function seededShuffle<T>(arr: T[], rng: () => number): T[] {
  const result = [...arr];
  for (let i = result.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    [result[i], result[j]] = [result[j], result[i]];
  }
  return result;
}

/**
 * Latin square order: stratified shuffle by difficulty tier.
 *
 * Tasks are grouped into difficulty tiers (easy: 1-2, medium: 3, hard: 4-5),
 * each tier is shuffled independently, then concatenated in tier order.
 * This ensures each repetition has a different task order while maintaining
 * balanced difficulty distribution.
 *
 * The seed is derived from (corpus, repetition, model, arm) to ensure
 * deterministic but different orderings per cell.
 */
export function latinSquareOrder(
  tasks: Task[],
  corpus: string,
  repetition: number,
  model: string = '',
  arm: string = '',
): Task[] {
  const seedStr = `${corpus}:${repetition}:${model}:${arm}`;
  const rng = mulberry32(hashSeed(seedStr));

  // Group by difficulty tier
  const easy = tasks.filter(t => t.difficulty <= 2);
  const medium = tasks.filter(t => t.difficulty === 3);
  const hard = tasks.filter(t => t.difficulty >= 4);

  // Shuffle each tier independently
  const shuffledEasy = seededShuffle(easy, rng);
  const shuffledMedium = seededShuffle(medium, rng);
  const shuffledHard = seededShuffle(hard, rng);

  // Concatenate: easy → medium → hard
  return [...shuffledEasy, ...shuffledMedium, ...shuffledHard];
}

/**
 * Alternate arm order based on repetition parity.
 * Odd repetitions: vectorcode first, then traditional.
 * Even repetitions: traditional first, then vectorcode.
 */
export function alternateArmOrder(repetition: number): ('vectorcode' | 'traditional')[] {
  if (repetition % 2 === 1) {
    return ['vectorcode', 'traditional'];
  } else {
    return ['traditional', 'vectorcode'];
  }
}
