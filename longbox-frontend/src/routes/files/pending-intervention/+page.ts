import { getPendingInterventions } from '$lib/api/postprocess';

export const load = async () => {
  const pending = await getPendingInterventions();
  return { pending };
};
