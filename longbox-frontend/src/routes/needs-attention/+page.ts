import { getPullFailures } from '$lib/api/needs_attention';
import { getPendingInterventions } from '$lib/api/postprocess';

export const load = async () => {
  const [pullFailures, pending] = await Promise.all([
    getPullFailures(),
    getPendingInterventions()
  ]);
  return { pullFailures, pending };
};
