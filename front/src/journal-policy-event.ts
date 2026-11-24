import type { HumanEvent } from "./human-model.ts";

/** Only the two policy registry mutations published for an authorized session. */
export function journalPolicyEventLabel(metadata: HumanEvent["metadata"]): string | null {
  if (metadata.source !== "gateway" || metadata.provenance !== "observed" ||
    metadata.coverage !== "native" || metadata.resource.kind !== "session" || metadata.decision !== null) return null;

  switch (metadata.kind) {
    case "control.egress_policy.replaced": return "Politique réseau remplacée";
    case "control.egress_policy.revoke_requested": return "Révocation de politique réseau demandée";
    default: return null;
  }
}

export function journalEgressEventPresentation(metadata: HumanEvent["metadata"]): { label: string; unknown: string } | null {
  if (metadata.source !== "gateway" || metadata.provenance !== "observed" ||
    metadata.coverage !== "partial" || metadata.resource.kind !== "session" || metadata.decision !== null) return null;

  switch (metadata.kind) {
    case "egress.decision":
      return { label: "Décision egress enregistrée", unknown: "Verdict non fourni dans les métadonnées" };
    case "egress.connection":
      return { label: "Résultat de connexion egress enregistré", unknown: "Résultat non fourni dans les métadonnées" };
    default: return null;
  }
}
