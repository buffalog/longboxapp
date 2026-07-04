// The reader fetches count / issue / progress itself on mount (it also wires
// browser-only listeners there), so the loader just surfaces the issue id.
export const load = ({ params }: { params: { issue_id: string } }) => {
  return { issueId: Number(params.issue_id) };
};
