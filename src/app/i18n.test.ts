import { describe, expect, it } from "vitest";
import { formatBytes, LOCALES } from "../locales/i18n";

describe("Codex Move i18n and formatting", () => {
  it("contains complete Chinese and English translations for all 10 content categories", () => {
    const categories = [
      "ActiveAndArchivedChats",
      "ChatMetadataAndIndex",
      "UserSkills",
      "SharedAgentSkills",
      "InstructionFiles",
      "CoreConfig",
      "ExtensionsMemoriesRulesHooks",
      "ChatAttachments",
      "ProjectFiles",
      "PluginManifests",
    ];
    for (const cat of categories) {
      expect(LOCALES["zh-CN"].categoryNames[cat]).toBeTruthy();
      expect(LOCALES["en"].categoryNames[cat]).toBeTruthy();
    }
  });

  it("formats byte counts accurately", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1048576)).toBe("1.0 MB");
  });
});
