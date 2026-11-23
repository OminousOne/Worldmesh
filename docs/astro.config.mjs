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
      logo: { src: './src/assets/logo.svg', replacesTitle: false },
      favicon: '/favicon.svg',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/OminousOne/Worldmesh' }],
      editLink: { baseUrl: 'https://github.com/OminousOne/Worldmesh/edit/main/docs/' },
      customCss: ['./src/styles/theme.css'],
      lastUpdated: true,
      plugins: [
        starlightLlmsTxt({
          projectName: 'Worldmesh',
          description:
            'Worldmesh is a Rust networking library for large multiplayer worlds. Games implement one trait (Game); Worldmesh runs it across Server Entity Groups (which simulate entities), Spatial Layer Cells (which own areas and answer "what\'s near me?"), an orchestrator and gateways, all over NATS. Browser clients use @worldmesh/client.',
          details:
            'For agents: start with the Game trait reference and the "Your first game" guide. Game code only runs in Server Entity Groups, inside Game::tick, on_command and on_action, through the Me handle. Actions are always delivered on the next tick, wherever the target is. Test game rules with worldmesh::testing::TestWorld (no NATS needed).',
          promote: ['getting-started/**', 'reference/game-trait', 'concepts/**'],
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
            { label: 'Game trait and Me', slug: 'reference/game-trait' },
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
