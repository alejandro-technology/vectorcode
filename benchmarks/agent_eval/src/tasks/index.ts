import { taskSymbolLookup } from './symbol-lookup.js';
import { taskArchTrace } from './arch-trace.js';
import { taskBugHunt } from './bug-hunt.js';
import { taskStatusCommand } from './status-command.js';
import { taskRefactorPlan } from './refactor-plan.js';
import { Task } from '../types.js';

export const tasks: Task[] = [
  taskSymbolLookup,
  taskArchTrace,
  taskBugHunt,
  taskStatusCommand,
  taskRefactorPlan
];
