// The Worldmesh documentation site: Astro Starlight, served from GitHub Pages at /Worldmesh.
// Also builds /llms.txt, /llms-full.txt and /llms-small.txt for AI agents.
import starlight from '@astrojs/starlight';
import { defineConfig } from 'astro/config';
import starlightLlmsTxt from 'starlight-llms-txt';

export default defineConfig({
  site: 'https://ominousone.github.io',
  base: '/Worldmesh',
  integrations: [
    starlight({
      title: 'Worldmesh',
      description: 'A Rust networking library that splits a large multiplayer world across many servers while players see one seamless world.',
      logo: { src: './src/assets/logo.png', alt: '', replacesTitle: false },
      favicon: '/favicon.ico',
      head: [
        { tag: 'link', attrs: { rel: 'icon', type: 'image/png', sizes: '192x192', href: '/Worldmesh/favicon.png' } },
        { tag: 'link', attrs: { rel: 'apple-touch-icon', href: '/Worldmesh/apple-touch-icon.png' } },
        { tag: 'meta', attrs: { name: 'theme-color', content: '#071f29' } },
      ],
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/OminousOne/Worldmesh' }],
      editLink: { baseUrl: 'https://github.com/OminousOne/Worldmesh/edit/main/docs/' },
      customCss: [
        '@fontsource-variable/unbounded',
        '@fontsource-variable/figtree',
        '@fontsource-variable/spline-sans-mono',
        './src/styles/theme.css',
      ],
      components: { Hero: './src/components/Hero.astro' },
      expressiveCode: {
        styleOverrides: {
          borderRadius: '10px',
          codeFontFamily: 'var(--__sl-font-mono)',
          uiFontFamily: 'var(--__sl-font)',
        },
      },
      lastUpdated: true,
      plugins: [
        starlightLlmsTxt({
          projectName: 'Worldmesh',
          description:
            'Worldmesh is a Rust networking library for large multiplayer worlds. It does the networking and none of the game: a game is a World built from plugins that register components, commands, actions, events, systems and kinds. Worldmesh runs it across Server Entity Groups (which simulate entities), Spatial Layer Cells (which own areas and answer "what\'s near me?"), an orchestrator and gateways, all over NATS. Browser clients use @worldmesh/client.',
          details:
            'For agents: start with the World reference and the "Your first game" guide. Game code only runs in Server Entity Groups, in systems and command/action handlers, through the Me handle, and only changes the entity it runs for. Actions are always delivered on the next tick, wherever the target is. Test game rules with worldmesh::testing::TestWorld (no NATS needed).',
          promote: ['getting-started/**', 'reference/world', 'concepts/**'],
        }),
      ],
      sidebar: [
        {
          label: 'Getting started',
          items: [
            { label: 'Introduction', slug: 'getting-started/introduction' },
            { label: 'Run the example', slug: 'getting-started/quickstart' },
            { label: 'Your first game', slug: 'getting-started/first-game' },
          ],
        },
        {
          label: 'Concepts',
          items: [
            { label: 'Plugins, components and messages', slug: 'concepts/plugins-and-components' },
            { label: 'Architecture', slug: 'concepts/architecture' },
            { label: 'Entities and actions', slug: 'concepts/entities-and-actions' },
            { label: 'Ticks, inputs and prediction', slug: 'concepts/ticks-and-inputs' },
            { label: '"What\'s near me?"', slug: 'concepts/spatial-queries' },
            { label: 'Scaling and handoffs', slug: 'concepts/scaling' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'Testing game rules', slug: 'guides/testing' },
            { label: 'The browser client', slug: 'guides/client' },
            { label: 'The debug panel', slug: 'guides/debug-panel' },
            { label: 'Running a world', slug: 'guides/deployment' },
            { label: 'Performance', slug: 'guides/performance' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'World, plugins and Me', slug: 'reference/world' },
            { label: 'Configuration and CLI', slug: 'reference/configuration' },
            { label: 'Wire protocol', slug: 'reference/protocol' },
            { label: 'TypeScript packages', slug: 'reference/typescript' },
            { label: 'For AI agents', slug: 'reference/agents' },
          ],
        },
      ],
    }),
  ],
});
