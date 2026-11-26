# Prepare a session and send an instruction

[Français](README.fr.md)

Use a dashboard and gateway configured for `owned_sources`. Sign in with a named account and select a personal context. Ask the operator for the opaque launch target identifier and its current revision. The context must grant each action independently. [Owned sources](../identity/OWNED_SOURCES.md)

1. Open **Nouvelle session IA**, enter the published target and revision, then select **Préparer le lancement**. Review the retained preparation, its expiry and its audit receipt. Preparing leaves the worker and quota untouched.
2. Select **Lancer la session** to commit that preparation. The acceptance receipt identifies the session and its named grant. Check the broker and runtime observations for admission and startup.
3. If the launch result is unknown, use **Réessayer le même lancement** from the same context. This reuses the operation and preparation already submitted. The control can return an existing receipt after the preparation expires. A refusal during reconciliation leaves the original outcome unknown.
4. **Envoyer une consigne** requires a current conversation binding for the exact session, worker, attempt and input epoch. Review the destination before sending. Read the retained receipt to distinguish recorded, transmitted, acknowledged and unknown delivery. Acknowledgment concerns the provider protocol. Observe any resulting action separately.

The current implementation covers the durable control-plane operations and the dashboard views. The broker composition exercises a controlled stdio child and a real owned Store in tests. The bridge from supervised production admission and the end-to-end runtime and provider paths remain open. [Session contract](../../engineering/SESSION_CONTROL.md) · [Qualification gates](../../engineering/PRODUCTION_READINESS.md)
