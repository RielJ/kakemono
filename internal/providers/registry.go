package providers

import (
	"math/rand"
	"time"
)

// Registry holds all registered providers, keyed by their Name().
type Registry struct {
	providers map[string]Provider
}

// NewRegistry creates an empty Registry. Providers are registered via Register.
// Provider construction and registration is the caller's responsibility, allowing
// this package to remain free of import cycles with its sub-packages.
func NewRegistry() *Registry {
	return &Registry{
		providers: make(map[string]Provider),
	}
}

// Register adds p to the registry under p.Name(). Any existing provider with
// the same name is silently replaced.
func (r *Registry) Register(p Provider) {
	r.providers[p.Name()] = p
}

// Get returns the provider registered under name and a boolean indicating
// whether such a provider exists.
func (r *Registry) Get(name string) (Provider, bool) {
	p, ok := r.providers[name]
	return p, ok
}

// Enabled returns all registered providers whose Enabled() method returns true,
// in non-deterministic map-iteration order.
func (r *Registry) Enabled() []Provider {
	result := make([]Provider, 0, len(r.providers))
	for _, p := range r.providers {
		if p.Enabled() {
			result = append(result, p)
		}
	}
	return result
}

// Random picks a random enabled provider from the subset named by names.
// If names is empty, all enabled providers are eligible.
// If none of the named providers is enabled, falls back to any enabled provider.
// Returns nil when no eligible provider is found.
func (r *Registry) Random(names []string) Provider {
	rng := rand.New(rand.NewSource(time.Now().UnixNano()))

	if len(names) > 0 {
		// Shuffle to avoid always preferring the first entry.
		shuffled := make([]string, len(names))
		copy(shuffled, names)
		rng.Shuffle(len(shuffled), func(i, j int) { shuffled[i], shuffled[j] = shuffled[j], shuffled[i] })

		var candidates []Provider
		for _, name := range shuffled {
			if p, ok := r.providers[name]; ok && p.Enabled() {
				candidates = append(candidates, p)
			}
		}
		if len(candidates) > 0 {
			return candidates[rng.Intn(len(candidates))]
		}
	}

	// Fall back to any enabled provider.
	all := r.Enabled()
	if len(all) == 0 {
		return nil
	}
	return all[rng.Intn(len(all))]
}
