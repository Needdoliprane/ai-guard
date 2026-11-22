// Presentation of the closed operation catalogue the Rust control serves with each named-human context.
// Pure: no DOM, no network, no clock. The catalogue describes the offer of the control and authorizes nothing:
// every route revalidates membership, OIDC ceiling and resource permissions, and a read can still be refused.
// No right is ever derived here from a capability list, a mode or an account name.
import type { HumanContext, HumanOperation, HumanOperationAccess, OperationAvailability, OperationPermission, OperationScope } from "./human-model.ts";

/** What kind of offer an operation is. Static fact of the identifier, not of the wire. */
export type OperationClass = "read_metadata" | "read_content" | "export" | "admin_read" | "mutation";
export interface OperationFacts {
  class: OperationClass;
  /** Each resource keeps its own permission: the context offer never opens a resource by itself. */
  resourceAcl: boolean;
  label: string; description: string;
}
export const OPERATION_FACTS: Readonly<Record<HumanOperation, OperationFacts>> = {
  resources_read: { class: "read_metadata", resourceAcl: true, label: "Sessions, agents, sandboxes et projets autorisés",
    description: "ressources listées avec leurs propres permissions de métadonnées, de contenu et d’export" },
  events_read_metadata: { class: "read_metadata", resourceAcl: true, label: "Événements des ressources autorisées",
    description: "métadonnées des événements ; chaque lecture laisse un reçu durable" },
  events_read_content: { class: "read_content", resourceAcl: true, label: "Contenu des événements",
    description: "contenu des événements ; chaque ressource garde sa propre permission de contenu" },
  events_export: { class: "export", resourceAcl: true, label: "Export d’une sélection",
    description: "export d’une sélection ; chaque ressource garde sa propre permission d’export" },
  events_stream: { class: "read_metadata", resourceAcl: true, label: "Suite en direct des événements",
    description: "événements autorisés enregistrés après la position lue" },
  summary_read: { class: "read_metadata", resourceAcl: false, label: "Synthèse du périmètre",
    description: "synthèse du périmètre de ce contexte" },
  brokers_read: { class: "admin_read", resourceAcl: false, label: "Registre des brokers du tenant",
    description: "baux, autorités déclarées et réceptions observées" },
  workers_read: { class: "admin_read", resourceAcl: false, label: "Registre des workers du tenant",
    description: "états du contrôle, dernière observation runtime et associations d’autorité" },
  federation_read: { class: "admin_read", resourceAcl: false, label: "Composition workers et brokers",
    description: "workers et brokers enregistrés, lus en deux snapshots indépendants" },
  quota_read: { class: "admin_read", resourceAcl: false, label: "Quotas du tenant",
    description: "quotas d’allocation du tenant" },
  usage_read: { class: "admin_read", resourceAcl: false, label: "Usage du tenant",
    description: "temps d’allocation relevé, qui n’est ni un prix ni une facture" },
  memberships_read: { class: "admin_read", resourceAcl: false, label: "Appartenances",
    description: "appartenances et contextes des comptes du tenant" },
  configuration_read: { class: "admin_read", resourceAcl: false, label: "Configuration effective",
    description: "configuration effective et sa provenance, sans credential" },
  memberships_manage: { class: "mutation", resourceAcl: false, label: "Gestion des appartenances",
    description: "modification ou retrait des droits d’une appartenance existante, audités avant effet ; la création reste à l’opérateur" },
  grants_manage: { class: "mutation", resourceAcl: true, label: "Gestion des partages",
    description: "partage nominatif d’une ressource existante, sans dépasser vos propres droits" },
  launch_approve: { class: "mutation", resourceAcl: true, label: "Approbation de lancement",
    description: "approbation d’un lancement demandé sur une ressource autorisée" },
  session_launch_prepare: { class: "mutation", resourceAcl: true, label: "Préparation d’un lancement",
    description: "préparation retenue d’un lancement de session" },
  session_launch: { class: "mutation", resourceAcl: true, label: "Lancement de session",
    description: "lancement de l’intention préparée exacte" },
  session_message_send: { class: "mutation", resourceAcl: true, label: "Consigne à une session",
    description: "envoi d’une courte consigne à une session existante" },
  session_messages_read: { class: "read_content", resourceAcl: true, label: "Échanges d’une session",
    description: "consignes et réponses d’une session ; chaque session garde sa propre permission de contenu" },
  session_attribution_read: { class: "read_metadata", resourceAcl: true, label: "Chaîne d’attribution d’une session",
    description: "initiateur, demande, lancement, agents et décisions liés à une session, sans contenu" },
  egress_policies_read: { class: "read_metadata", resourceAcl: true, label: "Politiques de sortie réseau",
    description: "destinations autorisées par session, échéances et révocations" },
  egress_attempts_read: { class: "read_metadata", resourceAcl: true, label: "Tentatives de sortie réseau",
    description: "décisions de sortie et connexions observées, distinctes l’une de l’autre" },
  egress_grants_read: { class: "read_metadata", resourceAcl: true, label: "Autorisations de sortie temporaires",
    description: "autorisations bornées actives, expirées ou révoquées" },
  egress_coverage_read: { class: "read_metadata", resourceAcl: true, label: "Couverture du contrôle des sorties",
    description: "chemins médiés, capteurs et chemins non couverts, nommés comme tels" },
  egress_summary_read: { class: "read_metadata", resourceAcl: true, label: "Synthèse des sorties réseau",
    description: "comptes des décisions et autorisations sur une période, calculés par le contrôle" },
  egress_grant_approve: { class: "mutation", resourceAcl: false, label: "Approbation d’une sortie réseau",
    description: "autorisation bornée d’une destination exacte, auditée avant effet" },
  egress_grant_revoke: { class: "mutation", resourceAcl: false, label: "Révocation d’une sortie réseau",
    description: "retrait d’une autorisation de sortie, audité avant effet" },
};

export const scopeText: Readonly<Record<OperationScope, string>> = {
  own_and_shared: "vos ressources et celles partagées avec vous",
  organization: "les ressources de l’organisation",
  tenant: "les registres du tenant",
};
export const permissionText: Readonly<Record<OperationPermission, string>> = {
  granted: "Accordé à ce contexte.",
  not_granted: "Non accordé à ce contexte : aucune requête n’est émise.",
  requires_administration: "Votre appartenance porte ce droit, mais seul son contexte d’administration l’offre : ouvrez le contexte d’administration de la même appartenance.",
  requires_personal: "Votre appartenance porte ce droit, mais seul son contexte personnel l’offre : ouvrez le contexte personnel de la même appartenance.",
};
/** Why a granted operation is or is not served. An unavailable read is never an empty one. */
export const availabilityText: Readonly<Record<OperationAvailability, string>> = {
  available: "Servi par ce build ; chaque requête est autorisée de nouveau par le contrôle et peut encore être refusée.",
  migration_required: "La base du tenant attend la migration explicite des droits humains par l’opérateur : aucune requête n’est émise et l’état reste inconnu.",
  profile_unavailable: "Le profil de sources adopté par le contrôle ne fournit pas cette lecture : aucune route historique n’est appelée, rien n’est déduit et l’activité reste inconnue.",
  not_in_this_build: "Ce build ne sert encore aucune route pour cette opération : rien n’est lu, et rien n’est présenté comme vide.",
};

/** The entry of this operation in the context catalogue, or null without a context. */
export function access(context: HumanContext | null, operation: HumanOperation): HumanOperationAccess | null {
  return context?.operations.find((entry) => entry.operation === operation) ?? null;
}
/** Presentation gating only: offered to this context and served. A false value means no request is sent. */
export function can(context: HumanContext | null, operation: HumanOperation): boolean {
  const entry = access(context, operation);
  return entry?.permission === "granted" && entry.availability === "available";
}
/** The reason a granted operation is not served, or null when it is served or not granted. */
export function grantedUnavailable(context: HumanContext | null, operation: HumanOperation): Exclude<OperationAvailability, "available"> | null {
  const entry = access(context, operation);
  return entry?.permission === "granted" && entry.availability !== "available" ? entry.availability : null;
}

/**
 * Whether a destination built on this operation is offered, and in which form. "view": granted and served, the view
 * reads. "explanation": granted, and this build has its route, but the store or the adopted source profile does not
 * serve it now: the destination says why and sends nothing. null: not granted, or no route in this build.
 */
export function destinationState(context: HumanContext | null, operation: HumanOperation): "view" | "explanation" | null {
  const entry = access(context, operation);
  if (entry?.permission !== "granted" || entry.availability === "not_in_this_build") return null;
  return entry.availability === "available" ? "view" : "explanation";
}

/** A destination built on several operations reads only when every one is served, and explains when one is offered. */
export function combinedDestinationState(context: HumanContext | null, operations: readonly HumanOperation[]): "view" | "explanation" | null {
  const states = operations.map((operation) => destinationState(context, operation));
  if (states.length && states.every((state) => state === "view")) return "view";
  return states.some((state) => state !== null) ? "explanation" : null;
}
/** The administration destinations of the 360° dashboard and the operations each one reads. */
export const ADMIN_DESTINATIONS = { memberships: ["memberships_read"], usage: ["quota_read", "usage_read"], configuration: ["configuration_read"] } as const satisfies
  Readonly<Record<string, readonly HumanOperation[]>>;
export type AdminDestination = keyof typeof ADMIN_DESTINATIONS;
export function adminDestinationState(context: HumanContext | null, destination: AdminDestination) {
  return combinedDestinationState(context, ADMIN_DESTINATIONS[destination]);
}

/** Four facts the person must be able to tell apart. */
export type AccessState = "granted" | "not_granted" | "elsewhere" | "unavailable";
export function accessState(entry: HumanOperationAccess | null): AccessState {
  if (!entry || entry.permission === "not_granted") return "not_granted";
  if (entry.permission !== "granted") return "elsewhere";
  return entry.availability === "available" ? "granted" : "unavailable";
}
export const accessStateLabel: Readonly<Record<AccessState, string>> = {
  granted: "Accordé", not_granted: "Non accordé", elsewhere: "Autre contexte", unavailable: "Indisponible",
};
/** One sentence naming the operation, its state, its scope or reason, and what is read. */
export function accessExplanation(context: HumanContext | null, operation: HumanOperation): string {
  if (!context) return "Aucun contexte sélectionné : aucune requête n’est émise.";
  const entry = access(context, operation);
  const facts = OPERATION_FACTS[operation];
  if (!entry || entry.permission === "not_granted") return `Opération ${operation} non accordée à ce contexte : aucune requête n’est émise.`;
  if (entry.permission !== "granted") return `Opération ${operation} non offerte par ce contexte. ${permissionText[entry.permission]}`;
  const scope = `portée : ${scopeText[entry.scope]}`;
  return entry.availability === "available" ? `Accordé par l’opération ${operation} (${scope}) : ${facts.description}.` :
    `Opération ${operation} accordée (${scope}), mais indisponible. ${availabilityText[entry.availability]}`;
}
