// Mirrors the IndexerSettings.test.ts template — see that file for the
// rationale behind the mock / fixture / fireEvent + waitFor shape.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '$lib/api/client';
import { createWebhook, deleteWebhook, updateWebhook, type Webhook } from '$lib/api/webhooks';
import WebhookSettings from './WebhookSettings.svelte';

vi.mock('$lib/api/webhooks', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/api/webhooks')>()),
  createWebhook: vi.fn(),
  updateWebhook: vi.fn(),
  deleteWebhook: vi.fn()
}));

function sampleWebhook(over: Partial<Webhook> = {}): Webhook {
  return {
    id: 1,
    name: 'Slack',
    url: 'https://hooks.slack.com/services/x',
    event_mask: 5,
    enabled: true,
    created_at: '2026-05-20T00:00:00',
    ...over
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('WebhookSettings', () => {
  it('lists the webhooks it was given', () => {
    render(WebhookSettings, { props: { webhooks: [sampleWebhook()] } });
    expect(screen.getByText('Slack')).toBeInTheDocument();
  });

  it('shows an empty state when there are no webhooks', () => {
    render(WebhookSettings, { props: { webhooks: [] } });
    expect(screen.getByText('No webhooks configured.')).toBeInTheDocument();
  });

  it('creates a webhook with the selected event bits', async () => {
    vi.mocked(createWebhook).mockResolvedValue(sampleWebhook({ id: 2, name: 'Ops' }));
    render(WebhookSettings, { props: { webhooks: [] } });

    await fireEvent.click(screen.getByRole('button', { name: '+ Add webhook' }));
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'Ops' } });
    await fireEvent.input(screen.getByLabelText('URL'), {
      target: { value: 'https://example.com/hook' }
    });
    // "Pull failed" is bit 2.
    await fireEvent.click(screen.getByLabelText('Pull failed'));
    await fireEvent.click(screen.getByRole('button', { name: 'Add webhook' }));

    await waitFor(() =>
      expect(createWebhook).toHaveBeenCalledWith(
        expect.objectContaining({
          name: 'Ops',
          url: 'https://example.com/hook',
          event_mask: 2
        })
      )
    );
    expect(await screen.findByText('Ops')).toBeInTheDocument();
  });

  it('edits a webhook', async () => {
    vi.mocked(updateWebhook).mockResolvedValue(sampleWebhook({ name: 'Slack prod' }));
    render(WebhookSettings, { props: { webhooks: [sampleWebhook()] } });

    await fireEvent.click(screen.getByRole('button', { name: 'Edit webhook Slack' }));
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'Slack prod' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(updateWebhook).toHaveBeenCalledTimes(1));
    const [id, input] = vi.mocked(updateWebhook).mock.calls[0]!;
    expect(id).toBe(1);
    expect(input.name).toBe('Slack prod');
    expect(await screen.findByText('Slack prod')).toBeInTheDocument();
  });

  it('deletes a webhook and drops it from the list', async () => {
    vi.mocked(deleteWebhook).mockResolvedValue(undefined);
    render(WebhookSettings, { props: { webhooks: [sampleWebhook()] } });

    await fireEvent.click(screen.getByRole('button', { name: 'Remove webhook Slack' }));
    await waitFor(() => expect(deleteWebhook).toHaveBeenCalledWith(1));
    await waitFor(() => expect(screen.queryByText('Slack')).not.toBeInTheDocument());
  });

  it('surfaces an ApiError when a mutation fails', async () => {
    vi.mocked(createWebhook).mockRejectedValue(
      new ApiError(409, 'conflict.webhook_exists', 'A webhook named "Slack" already exists.')
    );
    render(WebhookSettings, { props: { webhooks: [] } });

    await fireEvent.click(screen.getByRole('button', { name: '+ Add webhook' }));
    await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'Slack' } });
    await fireEvent.input(screen.getByLabelText('URL'), { target: { value: 'https://x' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Add webhook' }));
    expect(await screen.findByText(/already exists/)).toBeInTheDocument();
  });
});
