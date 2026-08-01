import type { Goal, KeyResult } from '../types';

function normalize(value: string): string {
  return value.replace(/\s+/g, '').toLowerCase();
}

function uniqueMatch<T>(items: T[], query: string | undefined, values: (item: T) => string[]): T | undefined {
  if (!query) return undefined;
  const needle = normalize(query);
  const exact = items.filter(item => values(item).some(value => normalize(value) === needle));
  if (exact.length === 1) return exact[0];
  const partial = items.filter(item => values(item).some(value => {
    const candidate = normalize(value);
    return candidate.includes(needle) || needle.includes(candidate);
  }));
  return partial.length === 1 ? partial[0] : undefined;
}

function matchKeyResult(items: KeyResult[], title: string | undefined): KeyResult | undefined {
  return uniqueMatch(items, title, item => [item.code, item.title, `${item.code}${item.title}`]);
}

export function resolveAiTarget(goals: Goal[], objectiveTitle?: string, keyResultTitle?: string): { goal?: Goal; keyResult?: KeyResult } {
  const activeGoals = goals.filter(goal => goal.status === 'active');
  const goal = uniqueMatch(activeGoals, objectiveTitle, item => [item.title]);
  if (goal) {
    return { goal, keyResult: matchKeyResult(goal.keyResults ?? [], keyResultTitle) };
  }

  if (!keyResultTitle) return {};
  const matches = activeGoals.flatMap(item => {
    const keyResult = matchKeyResult(item.keyResults ?? [], keyResultTitle);
    return keyResult ? [{ goal: item, keyResult }] : [];
  });
  return matches.length === 1 ? matches[0] : {};
}
