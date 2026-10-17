import { z } from 'zod';

export const SummaryTemplateId = z.enum([
  'general',
  'one-on-one',
  'standup',
  'sales',
  'interview',
]);
export type SummaryTemplateId = z.infer<typeof SummaryTemplateId>;

export interface SummaryTemplate {
  id: SummaryTemplateId;
  label: string;
  /** Extra instructions for the LLM; the JSON shape stays the same so storage/UI don't change. */
  guidance: string;
  /** Words that make a sentence more relevant for the extractive fallback. */
  focus: string[];
}

export const SUMMARY_TEMPLATES: readonly SummaryTemplate[] = [
  {
    id: 'general',
    label: 'General',
    guidance: 'Summarize the key points and decisions.',
    focus: ['decide', 'decided', 'agreed', 'plan', 'next'],
  },
  {
    id: 'one-on-one',
    label: '1:1',
    guidance:
      'This is a 1:1. Cover how the person is doing, wins, blockers, feedback given in either direction, and career/growth topics. Action items are commitments by either person.',
    focus: [
      'feedback',
      'blocker',
      'blocked',
      'growth',
      'career',
      'feel',
      'goal',
    ],
  },
  {
    id: 'standup',
    label: 'Standup',
    guidance:
      'This is a standup. In the summary, list per person: done since last time, doing next, blockers. Keep it terse. Action items are blockers needing follow-up.',
    focus: [
      'yesterday',
      'today',
      'blocked',
      'blocker',
      'done',
      'working',
      'finished',
    ],
  },
  {
    id: 'sales',
    label: 'Sales call',
    guidance:
      "This is a sales call. Cover the customer's needs and pain points, budget, timeline, decision makers, objections, competitors mentioned, and agreed next steps. Q&A should capture the customer's questions.",
    focus: [
      'budget',
      'price',
      'pricing',
      'timeline',
      'decision',
      'competitor',
      'contract',
      'need',
      'pain',
    ],
  },
  {
    id: 'interview',
    label: 'Interview',
    guidance:
      "This is a job interview. Summarize the candidate's background, strengths, concerns and notable answers. Q&A should pair interview questions with the candidate's answers. Do not make a hiring recommendation.",
    focus: [
      'experience',
      'project',
      'team',
      'role',
      'built',
      'led',
      'challenge',
    ],
  },
];

export const templateById = (id: string | undefined): SummaryTemplate =>
  SUMMARY_TEMPLATES.find((t) => t.id === id) ?? SUMMARY_TEMPLATES[0]!;
