# Daily Morning Briefing

Start every day knowing exactly what needs your attention. HiveMind OS checks your email and calendar each morning, then delivers a clear summary — before you've finished your coffee.

```mermaid
flowchart LR
    A["⏰ 9 AM trigger"] --> B["🤖 AI reads emails\n& calendar"] --> C["📋 Briefing\ndelivered"]
```

## What You'll Need

| Item | Details |
|------|---------|
| **Email connector** | Gmail, Microsoft 365, or any IMAP account |
| **Calendar connector** | Microsoft 365 or Gmail (optional but recommended) |
| **Time** | About 5 minutes |

---

## Step 1: Connect Your Email & Calendar

If you've already connected your email (for example, from the [Customer Support](/use-cases/customer-support) use case), you can skip this step.

Otherwise, go to **Settings → Connectors**, click **Add Connector**, and choose your email provider. Follow the prompts to authorize access.

::: tip
For the best briefing experience, connect your calendar too — same process, just select your calendar provider.
:::

## Step 2: Create the Workflow

1. Go to **Workflows** and click **New Workflow**.
2. Name it something like `Morning Briefing`.
3. Set the mode to **Background**.

### Add a Schedule Trigger

4. Click **Add Trigger** and select **Schedule**.
5. Set it to run on **weekdays at 9:00 AM**. In the schedule field, enter the cron expression: `0 9 * * 1-5` (don't worry — the app shows a plain-English preview like "Every weekday at 9:00 AM" so you can confirm it's right).

### Add the Step

6. Click **Add Step** and choose **Invoke Agent**.
7. You can use the default persona or create a dedicated one (something like "Executive Assistant"). Make sure your email and calendar connectors allow this persona — go to **Settings → Connectors**, edit the connector, and add the persona to its **Allowed Personas** list.
8. In the instructions, type:

> Read my recent emails and check my calendar for today. Summarize everything into a concise morning briefing organized by priority. Highlight anything urgent, list today's meetings, and suggest what I should focus on first.

9. Click **Save** and toggle the workflow to **Enabled**.

---

## What You'll See

Every morning, a notification appears with something like this:

> **☀️ Good Morning — Here's Your Briefing for Tuesday, March 18**
>
> **🔴 Urgent**
> - Email from Acme Corp: Contract renewal deadline is tomorrow. They need a signed copy by 5 PM.
>
> **📨 New Emails (12)**
> - 3 customer inquiries about pricing (forwarded to support)
> - Newsletter from Industry Weekly
> - Invoice from CloudHost Inc. — $247.00 due March 25
> - 8 others (low priority)
>
> **📅 Today's Meetings**
> - 10:00 AM — Team standup (30 min, Google Meet)
> - 1:00 PM — Client call with Acme Corp (45 min, Zoom)
> - 3:30 PM — Marketing review (1 hr, Conference Room B)
>
> **✅ Suggested Priorities**
> 1. Handle the Acme Corp contract before your 1 PM call
> 2. Review the CloudHost invoice
> 3. Prep talking points for the marketing review

---

## Make It Yours

### Change the Schedule

Edit the workflow trigger to adjust the time. Early riser? Set it to 7 AM. Want a weekend edition? Change the cron to include Saturday and Sunday.

### Create an End-of-Day Recap

Duplicate the workflow, change the schedule to 5 PM, and adjust the instructions to summarize what happened today and what's carrying over to tomorrow.

---

## Related

- [Customer Support](/use-cases/customer-support) — Auto-reply to customer emails
- [Meeting Prep](/use-cases/meeting-prep) — Get detailed prep briefs for every meeting
- [Connectors Guide](/guides/messaging-bridges) — Set up email, calendar, Slack, and more
- [Scheduling Guide](/guides/scheduling) — Advanced scheduling options and cron expressions
