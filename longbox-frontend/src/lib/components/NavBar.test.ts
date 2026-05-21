import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { readable } from 'svelte/store';
import NavBar from './NavBar.svelte';

// NavBar reads `$page` for the active-route highlight. `/files` is a
// Library child, so the mock also exercises the parent-active state.
vi.mock('$app/stores', () => ({
  page: readable({ url: new URL('http://localhost/files') })
}));

describe('NavBar', () => {
  it('keeps dropdown children hidden until the trigger is clicked', () => {
    render(NavBar);
    expect(screen.getByRole('button', { name: 'Library' })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Series' })).not.toBeInTheDocument();
  });

  it('opens a dropdown on click and reveals its children', async () => {
    render(NavBar);
    await fireEvent.click(screen.getByRole('button', { name: 'Library' }));
    expect(screen.getByRole('link', { name: 'Series' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Needs attention' })).toBeInTheDocument();
  });

  it('closes the open dropdown on Escape', async () => {
    render(NavBar);
    await fireEvent.click(screen.getByRole('button', { name: 'Library' }));
    expect(screen.getByRole('link', { name: 'Series' })).toBeInTheDocument();
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('link', { name: 'Series' })).not.toBeInTheDocument();
  });

  it('opens one dropdown at a time', async () => {
    render(NavBar);
    await fireEvent.click(screen.getByRole('button', { name: 'Library' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Releases' }));
    // Library's menu closed; Releases' menu open.
    expect(screen.queryByRole('link', { name: 'Series' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Calendar' })).toBeInTheDocument();
  });

  it('marks the parent dropdown active for a child route', () => {
    // `page` is mocked at /files — a Library child.
    render(NavBar);
    expect(screen.getByRole('button', { name: 'Library' })).toHaveAttribute(
      'aria-current',
      'true'
    );
    expect(screen.getByRole('button', { name: 'Releases' })).not.toHaveAttribute('aria-current');
  });
});
