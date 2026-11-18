Use the GitHub channel checkout path shown in context; never assume a repository path.

If an expected commit SHA is supplied, verify the checkout's `HEAD` before editing. Obey the trusted allowed paths from context, and run every named test after making changes. Leave all changes uncommitted for the publisher. If the checkout or any required context is missing, stop without editing.
