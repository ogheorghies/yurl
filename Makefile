VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
LAST_TAG := $(shell git describe --tags --abbrev=0 2>/dev/null || echo "")

# Flags (override from CLI or skill)
CONFIRM ?= n
FORCE_DEMO ?= 0
SKIP_README_CHECK ?= 0

.PHONY: test build demo bump check-readme publish tag release

test:
	cargo test

build:
	cargo build --release

# Rebuild demo gifs from all .tape files
# Rebuilds if any source or tape changed since the gif, or FORCE_DEMO=1
demo:
	@command -v vhs >/dev/null 2>&1 || { echo "error: vhs not installed"; exit 1; }
	@found=0; \
	for tape in demo/*.tape; do \
		gif=$${tape%.tape}.gif; \
		if [ "$(FORCE_DEMO)" = "1" ] || [ ! -f "$$gif" ] || \
		   [ -n "$$(find src/ Cargo.toml "$$tape" -newer "$$gif" 2>/dev/null)" ]; then \
			echo "recording $$tape -> $$gif"; \
			vhs "$$tape" || exit 1; \
			found=1; \
		else \
			echo "skip $$tape (gif up to date)"; \
		fi; \
	done

# Version bump: v=patch (default), v=minor, v=major, or v=X.Y.Z
bump:
ifndef v
	$(eval v := patch)
endif
	@cur=$(VERSION); \
	IFS='.' read -r ma mi pa <<< "$$cur"; \
	case "$(v)" in \
		patch) new="$$ma.$$mi.$$((pa + 1))";; \
		minor) new="$$ma.$$((mi + 1)).0";; \
		major) new="$$((ma + 1)).0.0";; \
		*) new="$(v)";; \
	esac; \
	echo "$$cur -> $$new"; \
	sed -i '' "s/^version = \"$$cur\"/version = \"$$new\"/" Cargo.toml; \
	cargo check 2>/dev/null

# Fail if src/ changed since last tag but README.md didn't
check-readme:
ifeq ($(SKIP_README_CHECK),1)
	@echo "readme check: skipped"
else
	@if [ -z "$(LAST_TAG)" ]; then \
		echo "readme check: no previous tag, skipping"; \
	elif git diff --quiet $(LAST_TAG) -- src/ Cargo.toml 2>/dev/null; then \
		echo "readme check: no code changes"; \
	elif ! git diff --quiet $(LAST_TAG) -- README.md 2>/dev/null; then \
		echo "readme check: ok"; \
	else \
		echo "error: src/ changed since $(LAST_TAG) but README.md did not"; \
		exit 1; \
	fi
endif

publish:
	cargo publish --dry-run
	@if [ "$(CONFIRM)" = "y" ]; then \
		echo "publishing..."; \
	else \
		printf "Publish $(VERSION) to crates.io? [y/N] "; \
		read ans; \
		[ "$$ans" = "y" ] || { echo "aborted"; exit 1; }; \
	fi
	cargo publish

tag:
	git tag v$(VERSION)
	@echo "tagged v$(VERSION) (push with: git push && git push --tags)"

# Full release pipeline
release: bump test demo build check-readme publish tag
	@echo "released v$(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')"
