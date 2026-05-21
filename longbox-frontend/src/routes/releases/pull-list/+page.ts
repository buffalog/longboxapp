import { getPullFailures } from '$lib/api/needs_attention';
import { listPullList } from '$lib/api/pull';

export const load = async () => {
  const [entries, pullFailures] = await Promise.all([listPullList(), getPullFailures()]);
  return { entries, pullFailureCount: pullFailures.length };
};
