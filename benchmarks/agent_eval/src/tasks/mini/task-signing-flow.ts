import { Task } from '../../types.js';

export const taskMiniSigningFlow: Task = {
  id: 'mini-signing-flow',
  name: 'Token Signing Flow Trace',
  prompt: 'In itsdangerous, trace the complete token signing and verification flow. Start from `Signer.sign()` through to timestamp encoding and signature generation. Identify the hash algorithm used and explain how tampering is detected.',
  corpus: 'mini',
  difficulty: 4,
  type: 'read',
  targetRepos: ['itsdangerous'],
  verify: async () => {
    return { success: true };
  }
};
