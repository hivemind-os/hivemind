#!/usr/bin/env node
/**
 * create-hivemind-plugin — Scaffold a new Hivemind connector plugin.
 *
 * Usage:
 *   npm create hivemind-plugin          (interactive)
 *   npm create hivemind-plugin my-plugin --template connector
 */

import { resolve, join } from "node:path";
import { existsSync, mkdirSync, readFileSync, writeFileSync, readdirSync, statSync, copyFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import prompts from "prompts";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const TEMPLATES_DIR = resolve(__dirname, "..", "templates");

interface Options {
  name: string;
  template: "connector" | "tool-pack";
  description: string;
  author: string;
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const nameArg = args.find((a) => !a.startsWith("--"));
  const templateArg = args
    .find((a) => a.startsWith("--template="))
    ?.split("=")[1];

  console.log("\n🐝 Create Hivemind Plugin\n");

  const response = await prompts(
    [
      {
        type: nameArg ? null : "text",
        name: "name",
        message: "Plugin name:",
        initial: "my-hivemind-plugin",
        validate: (v: string) =>
          /^[a-z0-9-]+$/.test(v) || "Use lowercase letters, numbers, and hyphens",
      },
      {
        type: templateArg ? null : "select",
        name: "template",
        message: "Template:",
        choices: [
          {
            title: "Connector (tools + background loop)",
            value: "connector",
            description: "Full connector with tools, config, auth, and polling loop",
          },
          {
            title: "Tool Pack (tools only)",
            value: "tool-pack",
            description: "Simpler plugin with just tools and config",
          },
        ],
      },
      {
        type: "text",
        name: "description",
        message: "Description:",
        initial: "A Hivemind connector plugin",
      },
      {
        type: "text",
        name: "author",
        message: "Author:",
        initial: "",
      },
    ],
    { onCancel: () => process.exit(1) },
  );

  const opts: Options = {
    name: nameArg ?? response.name,
    template: (templateArg as any) ?? response.template,
    description: response.description,
    author: response.author,
  };

  const targetDir = resolve(process.cwd(), opts.name);

  if (existsSync(targetDir)) {
    console.error(`\n❌ Directory "${opts.name}" already exists.\n`);
    process.exit(1);
  }

  // Copy template
  const templateDir = join(TEMPLATES_DIR, opts.template);
  if (!existsSync(templateDir)) {
    console.error(`\n❌ Template "${opts.template}" not found.\n`);
    process.exit(1);
  }

  copyDirRecursive(templateDir, targetDir);

  // Replace placeholders in all files
  replaceInDir(targetDir, {
    "{{name}}": opts.name,
    "{{description}}": opts.description,
    "{{author}}": opts.author,
    "{{year}}": new Date().getFullYear().toString(),
  });

  console.log(`\n✅ Created plugin in ./${opts.name}\n`);
  console.log("Next steps:");
  console.log(`  cd ${opts.name}`);
  console.log("  npm install");
  console.log("  npm run build");
  console.log("  npm test");
  console.log("");
  console.log("To link to Hivemind for local development:");
  console.log("  hivemind plugin link .");
  console.log("");
}

function copyDirRecursive(src: string, dest: string): void {
  mkdirSync(dest, { recursive: true });
  for (const entry of readdirSync(src)) {
    const srcPath = join(src, entry);
    const destPath = join(dest, entry);
    if (statSync(srcPath).isDirectory()) {
      copyDirRecursive(srcPath, destPath);
    } else {
      copyFileSync(srcPath, destPath);
    }
  }
}

function replaceInDir(
  dir: string,
  replacements: Record<string, string>,
): void {
  for (const entry of readdirSync(dir)) {
    const fullPath = join(dir, entry);
    if (statSync(fullPath).isDirectory()) {
      replaceInDir(fullPath, replacements);
    } else if (
      fullPath.endsWith(".ts") ||
      fullPath.endsWith(".json") ||
      fullPath.endsWith(".md")
    ) {
      let content = readFileSync(fullPath, "utf8");
      for (const [key, value] of Object.entries(replacements)) {
        content = content.replaceAll(key, value);
      }
      writeFileSync(fullPath, content);
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
