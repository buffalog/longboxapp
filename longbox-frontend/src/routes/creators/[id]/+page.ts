import { getCreator } from '$lib/api/creators';

export const load = async ({ params }) => {
  const creator = await getCreator(Number(params.id));
  return { creator };
};
