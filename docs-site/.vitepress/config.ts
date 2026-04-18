import { defineConfig } from "vitepress";
import { withMermaid } from "vitepress-plugin-mermaid";

export default withMermaid(
  defineConfig({
    title: "HiveMind OS",
    description:
      "Privacy-first AI agent that lives on your machine. Your data, your rules.",
    head: [
      ["link", { rel: "icon", href: "/favicon.png" }],
      ["meta", { name: "theme-color", content: "#7c3aed" }],
      [
        "meta",
        {
          property: "og:title",
          content: "HiveMind OS Documentation",
        },
      ],
      [
        "meta",
        {
          property: "og:description",
          content:
            "Privacy-first AI agent that lives on your machine. Your data, your rules.",
        },
      ],
    ],

    // Clean URLs without .html extension
    cleanUrls: true,

    themeConfig: {
      logo: "/logo.png",

      nav: [
        { text: "Get Started", link: "/getting-started/quickstart" },
        { text: "Use Cases", link: "/use-cases/" },
        { text: "Concepts", link: "/concepts/how-it-works" },
        { text: "Guides", link: "/guides/personas" },
        { text: "Examples", link: "/examples/custom-persona" },
      ],

      sidebar: {
        "/": [
          {
            text: "Getting Started",
            collapsed: false,
            items: [
              {
                text: "Installation",
                link: "/getting-started/installation",
              },
              {
                text: "Quickstart",
                link: "/getting-started/quickstart",
              },
              {
                text: "First Five Minutes",
                link: "/getting-started/first-five-minutes",
              },

            ],
          },
          {
            text: "Use Cases",
            collapsed: false,
            items: [
              {
                text: "Overview",
                link: "/use-cases/",
              },
              {
                text: "Customer Support",
                link: "/use-cases/customer-support",
              },
              {
                text: "Daily Briefing",
                link: "/use-cases/daily-briefing",
              },
              {
                text: "Content Creation",
                link: "/use-cases/content-creation",
              },
              {
                text: "Meeting Prep",
                link: "/use-cases/meeting-prep",
              },
            ],
          },
          {
            text: "Core Concepts",
            collapsed: false,
            items: [
              { text: "How It Works", link: "/concepts/how-it-works" },
              { text: "Personas", link: "/concepts/personas" },
              { text: "Workflows", link: "/concepts/workflows" },
              { text: "Bots", link: "/concepts/bots" },
              {
                text: "Providers & Models",
                link: "/concepts/providers-and-models",
              },
              {
                text: "Privacy & Security",
                link: "/concepts/privacy-and-security",
              },
              {
                text: "Knowledge Graph",
                link: "/concepts/knowledge-graph",
              },
              { text: "Tools & MCP", link: "/concepts/tools-and-mcp" },
              { text: "Agent Skills", link: "/concepts/skills" },
              {
                text: "Managed Runtimes",
                link: "/concepts/managed-runtimes",
              },
              {
                text: "Agentic Loops",
                link: "/concepts/agentic-loops",
              },
              {
                text: "Sessions & Conversations",
                link: "/concepts/sessions-and-conversations",
              },
            ],
          },
          {
            text: "User Guides",
            collapsed: false,
            items: [
              {
                text: "No-Code Guide",
                link: "/guides/no-code-guide",
              },
              { text: "Personas", link: "/guides/personas" },
              { text: "Workflows", link: "/guides/workflows" },
              { text: "Bots", link: "/guides/bots" },
              {
                text: "Configure Providers",
                link: "/guides/configure-providers",
              },
              {
                text: "Connectors",
                link: "/guides/messaging-bridges",
              },
              { text: "Scheduling", link: "/guides/scheduling" },
            ],
          },
          {
            text: "Advanced Guides",
            collapsed: true,
            items: [
              { text: "MCP Servers", link: "/guides/mcp-servers" },
              {
                text: "Security Policies",
                link: "/guides/security-policies",
              },
              {
                text: "Knowledge Management",
                link: "/guides/knowledge-management",
              },
              {
                text: "Agentic Loops",
                link: "/guides/agentic-loops",
              },
              {
                text: "Agents & Roles",
                link: "/guides/agents-and-roles",
              },
              { text: "Skills", link: "/guides/skills" },
              { text: "Local Models", link: "/guides/local-models" },
              { text: "Spatial Chat", link: "/guides/spatial-chat" },
            ],
          },
          {
            text: "CLI Reference",
            collapsed: true,
            items: [
              { text: "Overview", link: "/cli/overview" },
              { text: "Commands", link: "/cli/commands" },
            ],
          },
          {
            text: "Reference",
            collapsed: true,
            items: [
              {
                text: "Configuration",
                link: "/reference/configuration",
              },
              {
                text: "Slash Commands",
                link: "/reference/slash-commands",
              },
              {
                text: "Keyboard Shortcuts",
                link: "/reference/keyboard-shortcuts",
              },
            ],
          },
          {
            text: "Examples & Recipes",
            collapsed: true,
            items: [
              {
                text: "Custom Persona",
                link: "/examples/custom-persona",
              },
              {
                text: "Email Support Workflow",
                link: "/examples/pr-review-workflow",
              },
              {
                text: "Onboarding Chat Workflow",
                link: "/examples/chat-workflow-onboarding",
              },
              { text: "Bot Team", link: "/examples/bot-team" },
              {
                text: "Research Assistant",
                link: "/examples/research-assistant",
              },
              {
                text: "Daily Automation",
                link: "/examples/daily-automation",
              },
            ],
          },
          {
            text: "Help",
            collapsed: true,
            items: [
              { text: "Glossary", link: "/glossary" },
              { text: "FAQ", link: "/help/faq" },
              {
                text: "Troubleshooting",
                link: "/help/troubleshooting",
              },
            ],
          },
        ],
      },

      socialLinks: [
        {
          icon: "github",
          link: "https://github.com/hivemind-os/hivemind",
        },
      ],

      search: {
        provider: "local",
      },

      editLink: {
        pattern:
          "https://github.com/hivemind-os/hivemind/edit/main/docs-site/:path",
        text: "Edit this page on GitHub",
      },

      footer: {
        message: "Released under the MIT License.",
        copyright:
          'Copyright © 2024-present HiveMind OS Contributors · <a href="/privacy-policy">Privacy Policy</a>',
      },
    },

    // Mermaid plugin options
    mermaid: {
      theme: "dark",
    },
  })
);
