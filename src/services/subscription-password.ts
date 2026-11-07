const SUBSCRIPTION_PASSWORD_REQUIRED = 'PROFILE_SUBSCRIPTION_PASSWORD_REQUIRED'
const SUBSCRIPTION_PASSWORD_INCORRECT =
  'PROFILE_SUBSCRIPTION_PASSWORD_INCORRECT'

export function subscriptionPasswordFailure(
  err: unknown,
): 'required' | 'incorrect' | null {
  if (typeof err !== 'object' || err === null) return null
  const code = (err as { code?: unknown }).code
  if (code === SUBSCRIPTION_PASSWORD_REQUIRED) return 'required'
  if (code === SUBSCRIPTION_PASSWORD_INCORRECT) return 'incorrect'
  return null
}

interface PasswordRequest {
  readonly id: number
  readonly name: string
  readonly incorrect: boolean
  readonly resolve: (password: string | null) => void
}

const queue: PasswordRequest[] = []
const subscribers = new Set<() => void>()
let nextId = 0
const notify = () => subscribers.forEach((subscriber) => subscriber())

export const subscribeSubscriptionPassword = (subscriber: () => void) => {
  subscribers.add(subscriber)
  return () => {
    subscribers.delete(subscriber)
  }
}

export const getSubscriptionPasswordRequest = () => queue[0] ?? null

function requestSubscriptionPassword({
  name,
  incorrect,
}: {
  name: string
  incorrect: boolean
}): Promise<string | null> {
  return new Promise((resolve) => {
    queue.push({ id: nextId++, name, incorrect, resolve })
    if (queue.length === 1) notify()
  })
}

export function resolveSubscriptionPassword(password: string | null) {
  const request = queue.shift()
  if (!request) return
  notify()
  request.resolve(password)
}

export async function withSubscriptionPassword<T>(
  run: (password?: string) => Promise<T>,
  name: string,
): Promise<T | null> {
  let password: string | undefined
  while (true) {
    try {
      return await run(password)
    } catch (err) {
      const failure = subscriptionPasswordFailure(err)
      if (!failure) throw err
      const answer = await requestSubscriptionPassword({
        name,
        incorrect: failure === 'incorrect',
      })
      if (answer === null) return null
      password = answer
    }
  }
}
