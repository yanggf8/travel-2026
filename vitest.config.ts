import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    // Integration/regression-only tests (no unit test suite)
    include: ['tests/integration/**/*.test.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'html', 'lcov'],
      // Keep coverage focused on the integration surface we care about.
      include: ['src/state/**/*.ts'],
      exclude: [
        'src/**/*.d.ts',
        'src/_deprecated/**',
        'src/process/**', // legacy path (kept for safety)
      ],
    },
  },
});
