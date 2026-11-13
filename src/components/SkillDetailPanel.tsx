import { useEffect, useRef, useState } from "react";
import {
  Folder,
  ChevronDown,
  ChevronUp,
  Github,
  HardDrive,
  Globe,
  Languages,
  Loader2,
  StickyNote,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "../utils";
import {
  getSkillDocument,
  getSourceSkillDocument,
  getSkillSourceDiff,
  setSkillNotes,
  translateText,
  type ManagedSkill,
  type Project,
  type SkillDocument,
  type SourceSkillDocument,
  type SkillSourceDiff,
  type SkillToolToggle,
  type ToolInfo,
} from "../lib/tauri";
import { getErrorMessage, getErrorKind } from "../lib/error";
import { SkillSourceDiffViewer } from "./SkillSourceDiffViewer";
import { DetailSheet } from "./DetailSheet";
import { SkillMarkdown } from "./SkillMarkdown";
import { AgentToggleSection, type AgentToggleItem } from "./AgentToggleSection";
import { SkillProjectsSection } from "./SkillProjectsSection";
import { SyncDots } from "./SyncDots";

interface Props {
  skill: ManagedSkill | null;
  onClose: () => void;
  tools?: ToolInfo[];
  toolToggles?: SkillToolToggle[] | null;
  togglingTool?: string | null;
  onToggleTool?: (tool: string, enabled: boolean) => void;
  /** Whether `toolToggles` belong to the active preset, the only one that deploys. */
  isActivePreset: boolean;
  projects?: Project[];
  onProjectsChanged?: () => void;
  onNotesChanged?: () => void;
}

/** Toast message for a failed translation, mapping the "not configured" case
 *  to a localized hint instead of the backend's English message. */
const translateErrorMessage = (
  error: unknown,
  t: (key: string) => string
): string =>
  getErrorKind(error) === "invalid_input"
    ? t("mySkills.translate.notConfigured")
    : getErrorMessage(error, t("common.error"));

export function SkillDetailPanel({
  skill,
  onClose,
  tools,
  toolToggles,
  togglingTool,
  onToggleTool,
  isActivePreset,
  projects,
  onProjectsChanged,
  onNotesChanged,
}: Props) {
  if (!skill) return null;

  const panelKey = [
    skill.id,
    skill.updated_at,
    skill.source_type,
    skill.source_ref ?? "",
    skill.source_revision ?? "",
    skill.remote_revision ?? "",
  ].join(":");

  return (
    <SkillDetailPanelContent
      key={panelKey}
      skill={skill}
      onClose={onClose}
      tools={tools}
      toolToggles={toolToggles}
      togglingTool={togglingTool}
      onToggleTool={onToggleTool}
      isActivePreset={isActivePreset}
      projects={projects}
      onProjectsChanged={onProjectsChanged}
      onNotesChanged={onNotesChanged}
    />
  );
}

function SkillDetailPanelContent({
  skill,
  onClose,
  tools,
  toolToggles,
  togglingTool,
  onToggleTool,
  isActivePreset,
  projects,
  onProjectsChanged,
  onNotesChanged,
}: {
  skill: ManagedSkill;
  onClose: () => void;
  tools?: ToolInfo[];
  toolToggles?: SkillToolToggle[] | null;
  togglingTool?: string | null;
  onToggleTool?: (tool: string, enabled: boolean) => void;
  /** Whether `toolToggles` belong to the active preset, the only one that deploys. */
  isActivePreset: boolean;
  projects?: Project[];
  onProjectsChanged?: () => void;
  onNotesChanged?: () => void;
}) {
  const { t } = useTranslation();
  const [doc, setDoc] = useState<SkillDocument | null>(null);
  const [sourceDoc, setSourceDoc] = useState<SourceSkillDocument | null>(null);
  const [sourceDiff, setSourceDiff] = useState<SkillSourceDiff | null>(null);
  const [sourceDiffFailed, setSourceDiffFailed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [isMetadataExpanded, setIsMetadataExpanded] = useState(false);
  const [contentTab, setContentTab] = useState<"local" | "diff" | "source">("local");
  // Notes (user annotations on the skill, persisted through set_skill_notes).
  const [notesEditing, setNotesEditing] = useState(false);
  const [notesDraft, setNotesDraft] = useState("");
  const [notesSaving, setNotesSaving] = useState(false);
  // One-click translations: the short description and the full document.
  const [descTranslated, setDescTranslated] = useState<string | null>(null);
  const [descTranslating, setDescTranslating] = useState(false);
  const [docTranslated, setDocTranslated] = useState<string | null>(null);
  const [docTranslating, setDocTranslating] = useState(false);
  const localRequestIdRef = useRef(0);
  const sourceRequestIdRef = useRef(0);
  const diffRequestedRef = useRef(false);
  const skillId = skill.id;
  const supportsSourceDiff =
    skill.source_type === "git"
    || skill.source_type === "skillssh"
    || ((skill.source_type === "local" || skill.source_type === "import") && !!skill.source_ref);
  const [sourceLoading, setSourceLoading] = useState(supportsSourceDiff);
  const localDocVersion = `${skill.id}:${skill.updated_at}`;
  const sourceDocVersion = [
    skill.id,
    skill.source_type,
    skill.source_ref ?? "",
    skill.source_ref_resolved ?? "",
    skill.source_revision ?? "",
    skill.remote_revision ?? "",
  ].join(":");

  useEffect(() => {
    localRequestIdRef.current += 1;
    const requestId = localRequestIdRef.current;

    getSkillDocument(skillId)
      .then((nextDoc) => {
        if (requestId === localRequestIdRef.current) {
          setDoc(nextDoc);
        }
      })
      .catch(() => {
        if (requestId === localRequestIdRef.current) {
          setDoc(null);
        }
      })
      .finally(() => {
        if (requestId === localRequestIdRef.current) {
          setLoading(false);
        }
      });
  }, [skillId, localDocVersion]);

  useEffect(() => {
    if (!supportsSourceDiff) {
      return;
    }

    sourceRequestIdRef.current += 1;
    const requestId = sourceRequestIdRef.current;

    getSourceSkillDocument(skillId)
      .then((nextDoc) => {
        if (requestId === sourceRequestIdRef.current) {
          setSourceDoc(nextDoc);
        }
      })
      .catch(() => {
        if (requestId === sourceRequestIdRef.current) {
          setSourceDoc(null);
        }
      })
      .finally(() => {
        if (requestId === sourceRequestIdRef.current) {
          setSourceLoading(false);
        }
      });
  }, [skillId, supportsSourceDiff, sourceDocVersion]);

  // Lazily load the whole-directory diff only when the user opens the Diff
  // tab. For git/skills.sh skills this clones the repo, so we avoid paying
  // that cost (and a second clone alongside the source doc) up front.
  useEffect(() => {
    if (contentTab !== "diff" || !supportsSourceDiff) return;
    if (diffRequestedRef.current) return;
    diffRequestedRef.current = true;

    getSkillSourceDiff(skillId)
      .then((diff) => setSourceDiff(diff))
      .catch(() => setSourceDiffFailed(true));
  }, [contentTab, supportsSourceDiff, skillId]);

  const handleSaveNotes = async () => {
    setNotesSaving(true);
    try {
      await setSkillNotes(skillId, notesDraft);
      toast.success(t("mySkills.notes.saved"));
      setNotesEditing(false);
      onNotesChanged?.();
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    } finally {
      setNotesSaving(false);
    }
  };

  const handleTranslateDescription = async () => {
    if (!skill.description) return;
    setDescTranslating(true);
    try {
      setDescTranslated(await translateText(skill.description));
    } catch (error) {
      toast.error(translateErrorMessage(error, t));
    } finally {
      setDescTranslating(false);
    }
  };

  const handleTranslateDoc = async () => {
    if (!activeDoc) return;
    setDocTranslating(true);
    try {
      setDocTranslated(await translateText(activeDoc.content));
    } catch (error) {
      toast.error(translateErrorMessage(error, t));
    } finally {
      setDocTranslating(false);
    }
  };

  const sourceIcon = (type: string) => {
    switch (type) {
      case "git":
      case "skillssh":
        return <Github className="h-3.5 w-3.5" />;
      case "local":
      case "import":
        return <HardDrive className="h-3.5 w-3.5" />;
      default:
        return <Globe className="h-3.5 w-3.5" />;
    }
  };

  const sourceTypeLabel = (type: string) => (type === "skillssh" ? "skills.sh" : type);

  const metadataItems = [
    { label: t("mySkills.sourceType"), value: sourceTypeLabel(skill.source_type) },
    { label: t("mySkills.sourceRef"), value: skill.source_ref },
    { label: t("mySkills.sourceResolved"), value: skill.source_ref_resolved },
    { label: t("mySkills.sourceBranch"), value: skill.source_branch },
    { label: t("mySkills.sourceSubpath"), value: skill.source_subpath },
    { label: t("mySkills.sourceRevision"), value: skill.source_revision },
  ].filter((item) => Boolean(item.value));

  const activeDoc = doc?.skill_id === skill.id ? doc : null;
  const activeSourceDoc = sourceDoc?.skill_id === skill.id ? sourceDoc : null;
  const activeSourceDiff = sourceDiff?.skill_id === skill.id ? sourceDiff : null;
  const sourceDiffLoading =
    contentTab === "diff" && supportsSourceDiff && !activeSourceDiff && !sourceDiffFailed;
  const toggleItems: AgentToggleItem[] = (toolToggles ?? []).map((toggle) => ({
    key: toggle.tool,
    displayName: toggle.display_name,
    enabled: toggle.enabled,
    deployed: toggle.deployed,
    isAvailable: toggle.installed && toggle.globally_enabled,
    disabled: !toggle.installed || !toggle.globally_enabled,
    badgeLabel: !toggle.installed
      ? t("mySkills.agentToggleNotInstalled")
      : !toggle.globally_enabled
        ? t("mySkills.agentToggleDisabledGlobally")
        : null,
  }));

  const meta = (
    <>
      <div className="flex flex-wrap items-center gap-2 text-[12px] text-muted">
        {tools && <SyncDots skill={skill} tools={tools} size="sm" includeOrphan />}
        {skill.tags.length > 0 && (
          <>
            {tools && <span className="mx-0.5 h-3 w-px bg-border-subtle" />}
            {skill.tags.map((tag) => (
              <span
                key={tag}
                className="inline-flex items-center rounded-full bg-surface-hover px-2 py-0.5 text-[11px] font-medium text-secondary"
              >
                {tag}
              </span>
            ))}
          </>
        )}
      </div>
      <div className="mt-3 flex min-w-0 items-center gap-2 text-[13px] text-muted">
        <Folder className="h-3.5 w-3.5 shrink-0" />
        <span className="font-mono truncate" title={skill.central_path}>
          {skill.central_path}
        </span>
      </div>
      <div className="mt-3 rounded-xl border border-border-subtle bg-surface/70">
        <div className="flex items-center justify-between px-4 py-2">
          <span className="flex items-center gap-1.5 text-[12px] font-medium text-muted">
            <StickyNote className="h-3.5 w-3.5" />
            {t("mySkills.notes.title")}
          </span>
          {!notesEditing && (
            <button
              type="button"
              onClick={() => {
                setNotesDraft(skill.notes ?? "");
                setNotesEditing(true);
              }}
              className="text-[12px] text-muted transition-colors hover:text-secondary"
            >
              {skill.notes ? t("mySkills.notes.edit") : t("mySkills.notes.add")}
            </button>
          )}
        </div>
        {notesEditing ? (
          <div className="px-4 pb-3">
            <textarea
              value={notesDraft}
              onChange={(e) => setNotesDraft(e.target.value)}
              placeholder={t("mySkills.notes.placeholder")}
              rows={4}
              autoFocus
              className="w-full resize-y rounded-lg border border-border-subtle bg-background px-3 py-2 text-[13px] leading-6 text-secondary outline-none focus:border-accent"
            />
            <div className="mt-2 flex items-center gap-2">
              <button
                type="button"
                onClick={handleSaveNotes}
                disabled={notesSaving}
                className="rounded-full bg-accent px-3 py-1.5 text-[12px] font-medium text-white transition-opacity disabled:opacity-50"
              >
                {notesSaving ? t("common.loading") : t("common.save")}
              </button>
              <button
                type="button"
                onClick={() => setNotesEditing(false)}
                className="rounded-full bg-surface-hover px-3 py-1.5 text-[12px] font-medium text-muted transition-colors hover:text-secondary"
              >
                {t("common.cancel")}
              </button>
            </div>
          </div>
        ) : skill.notes ? (
          <p className="whitespace-pre-wrap px-4 pb-3 text-[13px] leading-6 text-secondary">
            {skill.notes}
          </p>
        ) : null}
      </div>
      {metadataItems.length > 0 && (
        <div className="mt-4 rounded-xl border border-border-subtle bg-surface/70">
          <button
            type="button"
            onClick={() => setIsMetadataExpanded((prev) => !prev)}
            aria-expanded={isMetadataExpanded}
            aria-controls="skill-source-metadata"
            className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left"
          >
            <span className="flex min-w-0 items-center gap-2">
              <span className="inline-flex shrink-0 items-center gap-1.5 rounded-full border border-border-subtle bg-bg-secondary px-2 py-1 text-[12px] text-muted">
                {sourceIcon(skill.source_type)}
                {sourceTypeLabel(skill.source_type)}
              </span>
              <span className="truncate text-[13px] font-medium text-secondary">
                {t("mySkills.sourceType")}
              </span>
            </span>
            <span className="inline-flex shrink-0 items-center gap-1 text-[12px] text-muted">
              <span>
                {isMetadataExpanded
                  ? t("mySkills.collapseAgentToggles")
                  : t("mySkills.expandAgentToggles")}
              </span>
              {isMetadataExpanded ? (
                <ChevronUp className="h-3.5 w-3.5" />
              ) : (
                <ChevronDown className="h-3.5 w-3.5" />
              )}
            </span>
          </button>
          {isMetadataExpanded && (
            <div id="skill-source-metadata" className="border-t border-border-subtle px-4 py-3">
              <div className="grid gap-2 md:grid-cols-2">
                {metadataItems.map((item) => (
                  <div key={item.label} className="min-w-0">
                    <div className="text-[11px] font-medium uppercase tracking-[0.08em] text-faint">
                      {item.label}
                    </div>
                    <div
                      className="mt-0.5 truncate font-mono text-[13px] text-secondary"
                      title={item.value ?? undefined}
                    >
                      {item.value}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </>
  );

  return (
    <DetailSheet
      open={true}
      title={skill.name}
      description={
        skill.description ? (
          <div className="flex items-start gap-2">
            <p className="line-clamp-3 min-w-0 flex-1">
              {descTranslated ?? skill.description}
            </p>
            <button
              type="button"
              onClick={() =>
                descTranslated
                  ? setDescTranslated(null)
                  : void handleTranslateDescription()
              }
              disabled={descTranslating}
              title={t("mySkills.translate.buttonTitle")}
              className="inline-flex shrink-0 items-center gap-1 rounded-full border border-border-subtle bg-surface px-2 py-1 text-[12px] font-medium text-muted transition-colors hover:text-secondary disabled:opacity-60"
            >
              {descTranslating ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Languages className="h-3 w-3" />
              )}
              {descTranslated
                ? t("mySkills.translate.original")
                : t("mySkills.translate.button")}
            </button>
          </div>
        ) : undefined
      }
      meta={meta}
      onClose={onClose}
    >
      {toolToggles && onToggleTool && (
        <AgentToggleSection
          items={toggleItems}
          togglingKey={togglingTool}
          onToggle={onToggleTool}
          showPendingSync={isActivePreset}
          className="mb-4"
        />
      )}

      {projects && projects.length > 0 && (
        <SkillProjectsSection
          skill={skill}
          projects={projects}
          onChanged={onProjectsChanged}
        />
      )}

      {supportsSourceDiff && (
        <div className="mb-4 flex flex-wrap items-center gap-2">
          {(["local", "diff", "source"] as const).map((tab) => (
            <button
              key={tab}
              type="button"
              onClick={() => setContentTab(tab)}
              className={cn(
                "rounded-full px-3 py-1.5 text-[12px] font-medium transition-colors",
                contentTab === tab
                  ? "bg-accent text-white"
                  : "bg-surface-hover text-muted hover:text-secondary"
              )}
              disabled={tab === "source" && sourceLoading}
            >
              {tab === "local"
                ? t("mySkills.docTabs.local")
                : tab === "diff"
                  ? t("mySkills.docTabs.diff")
                  : t("mySkills.docTabs.source")}
            </button>
          ))}
          {activeSourceDoc && (
            <span className="rounded-full border border-border-subtle bg-surface px-2 py-1 text-[12px] text-muted">
              {activeSourceDoc.source_label} · {activeSourceDoc.revision.slice(0, 7)}
            </span>
          )}
        </div>
      )}

      {contentTab === "local" && activeDoc && (
        <div className="mb-3 flex justify-end">
          <button
            type="button"
            onClick={() =>
              docTranslated ? setDocTranslated(null) : void handleTranslateDoc()
            }
            disabled={docTranslating}
            className={cn(
              "inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 text-[12px] font-medium transition-colors disabled:opacity-60",
              docTranslated
                ? "bg-surface-hover text-muted hover:text-secondary"
                : "bg-accent text-white"
            )}
          >
            {docTranslating ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <Languages className="h-3 w-3" />
            )}
            {docTranslating
              ? t("mySkills.translate.translating")
              : docTranslated
                ? t("mySkills.translate.original")
                : t("mySkills.translate.docButton")}
          </button>
        </div>
      )}

      {loading ? (
        <div className="mt-12 text-center text-[13px] text-muted">{t("common.loading")}</div>
      ) : contentTab === "diff" ? (
        sourceDiffLoading ? (
          <div className="mt-12 text-center text-[13px] text-muted">{t("common.loading")}</div>
        ) : activeSourceDiff ? (
          <SkillSourceDiffViewer entries={activeSourceDiff.entries} />
        ) : sourceDiffFailed ? (
          <div className="mt-12 text-center text-[13px] text-muted">{t("mySkills.sourceDiffUnavailable")}</div>
        ) : (
          <div className="mt-12 text-center text-[13px] text-muted">{t("common.loading")}</div>
        )
      ) : contentTab === "source" ? (
        sourceLoading ? (
          <div className="mt-12 text-center text-[13px] text-muted">{t("common.loading")}</div>
        ) : activeSourceDoc ? (
          <SkillMarkdown content={activeSourceDoc.content} />
        ) : (
          <div className="mt-12 text-center text-[13px] text-muted">{t("mySkills.sourceDiffUnavailable")}</div>
        )
      ) : activeDoc ? (
        <SkillMarkdown content={docTranslated ?? activeDoc.content} />
      ) : (
        <div className="mt-12 text-center text-[13px] text-muted">{t("common.documentMissing")}</div>
      )}
    </DetailSheet>
  );
}
