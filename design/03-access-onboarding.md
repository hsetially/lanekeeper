# 03: Access and onboarding

## Prompt

```
Using the Lanekeeper design system, design the access and onboarding screens:

1. Sign-in: a single "Sign in with Microsoft" button, the product name, and a one-line description. No other fields.
2. Access requested (Sam Okafor, pending): explains that an admin must approve access, shows when the request was sent, and has a "Sign out" link. Nothing else in the app is reachable.
3. Access disabled: clear message and who to contact.
4. GitHub token onboarding (Marco Diaz, Editor, first sign-in after approval):
   - Explain why a fine-grained token is needed (PRs are raised as you).
   - Show the exact settings: fine-grained only, repositories configuration-base-saas and csp-tenant-data, Contents read/write, Pull requests read/write, an expiry date.
   - Token input (masked), "Verify and save".
   - States:
     - verifying;
     - success (shows linked GitHub login "mdiaz" and expiry date);
     - rejected classic token ("Classic tokens aren't allowed by policy");
     - missing repo access (lists which repo);
     - login mismatch ("This token belongs to a different GitHub account than the one linked to you").
5. My GitHub token page (later visits): status, linked login, expires in 5 days (warning banner), replace, delete.
```

## Follow-ups

- "Make the onboarding a single focused page, not a wizard. A user should finish it in under a minute."
- "Show the expiry warning as it appears in the app shell banner."

## Done when

- [ ] Every state above is designed, in light and dark.
- [ ] The token is never shown in plain text after saving. Only the login and the expiry appear.
