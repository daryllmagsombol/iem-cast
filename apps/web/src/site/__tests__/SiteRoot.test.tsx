import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { SiteRoot } from '../SiteRoot';

test('unknown_hash_route_falls_back_to_documentation', () => {
  window.location.hash = '#/not-a-real-route';
  render(<SiteRoot />);
  expect(screen.getByRole('heading', { level: 1, name: /documentation/i })).toBeInTheDocument();
});

test('demo_hash_route_renders_the_silent_simulated_demo', () => {
  window.location.hash = '#/demo';
  render(<SiteRoot />);
  expect(screen.getByText(/simulated · silent demo · no host connection/i)).toBeInTheDocument();
});

test('site_exposes_theme_choice_and_source_link', () => {
  window.location.hash = '#/docs';
  render(<SiteRoot />);
  expect(screen.getByRole('button', { name: /theme/i })).toBeInTheDocument();
  expect(screen.getAllByRole('link', { name: /source/i }).length).toBeGreaterThan(0);
});
