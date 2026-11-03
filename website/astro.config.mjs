// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
import starlightLlmsTxt from "starlight-llms-txt";

export default defineConfig({
  site: "https://goldziher.github.io",
  base: "/skif",
  integrations: [
    starlight({
      title: "skif",
      description:
        "A dependency and package-manager linter, auditor and enforcer for JavaScript and TypeScript, " +
        "with first-class monorepo support. Manifest, workspace, version, catalog and unused-dependency " +
        "rules in one Rust binary and one skif.toml.",
      logo: {
        src: "./src/assets/logo.svg",
        alt: "skif",
        replacesTitle: true,
      },
      favicon: "/favicon.svg",
      customCss: ["./src/styles/custom.css"],
      social: [{ icon: "github", label: "GitHub", href: "https://github.com/Goldziher/skif" }],
      editLink: {
        baseUrl: "https://github.com/Goldziher/skif/edit/main/website/",
      },
      head: [
        {
          tag: "link",
          attrs: { rel: "apple-touch-icon", href: "/skif/apple-touch-icon.png" },
        },
        {
          tag: "link",
          attrs: { rel: "icon", type: "image/png", sizes: "32x32", href: "/skif/favicon-32.png" },
        },
        {
          tag: "meta",
          attrs: { property: "og:image", content: "https://goldziher.github.io/skif/og.png" },
        },
        {
          tag: "meta",
          attrs: { name: "twitter:card", content: "summary_large_image" },
        },
        {
          tag: "meta",
          attrs: { name: "twitter:image", content: "https://goldziher.github.io/skif/og.png" },
        },
      ],
      plugins: [
        starlightLinksValidator({ errorOnRelativeLinks: false }),
        starlightLlmsTxt({
          exclude: ["reference/cli", "reference/configuration", "reference/architecture", "rules/**"],
          promote: ["index*", "start/**", "guides/**"],
          minify: { collapseCodeBlocks: true },
          details:
            "skif lints package.json manifests, workspaces, version and catalog policies, and unused " +
            "dependencies in JavaScript and TypeScript monorepos, standalone packages, and several projects in one run (-r). Configuration is TOML only " +
            "(skif.toml or .skif.toml). Project source: https://github.com/Goldziher/skif",
        }),
      ],
      sidebar: [
        {
          label: "Start here",
          items: [
            { label: "Introduction", slug: "start/introduction" },
            { label: "Installation", slug: "start/installation" },
            { label: "Quickstart", slug: "start/quickstart" },
          ],
        },
        {
          label: "Guides",
          items: [
            { label: "Single packages and monorepos", slug: "guides/workspaces" },
            { label: "Integrations", slug: "guides/integrations" },
            { label: "Library usage", slug: "guides/library" },
            { label: "Comparison", slug: "guides/comparison" },
          ],
        },
        {
          label: "Rules",
          items: [
            { label: "Overview", slug: "rules" },
            { label: "Manifest", slug: "rules/manifest" },
            { label: "Workspace", slug: "rules/workspace" },
            { label: "Policy", slug: "rules/policy" },
            { label: "Catalog", slug: "rules/catalog" },
            { label: "JavaScript analysis", slug: "rules/js" },
          ],
        },
        {
          label: "Reference",
          items: [
            { label: "CLI", slug: "reference/cli" },
            { label: "Configuration", slug: "reference/configuration" },
            { label: "Architecture", slug: "reference/architecture" },
            { label: "Attributions", slug: "reference/attributions" },
          ],
        },
      ],
    }),
  ],
});
