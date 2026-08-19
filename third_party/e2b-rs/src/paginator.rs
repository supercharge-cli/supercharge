//! Cursor-based pagination state shared by list endpoints.

/// Shared pagination bookkeeping (`has_next` + `next_token`), mirroring the JS
/// `Paginator` base. Concrete list types own an instance and call
/// [`PaginationState::update_from_token`] after fetching each page.
#[derive(Debug, Clone)]
pub struct PaginationState {
    has_next: bool,
    next_token: Option<String>,
    limit: Option<u32>,
}

impl PaginationState {
    /// Create state for a fresh paginator. `has_next` starts `true` so the
    /// first page is always fetched.
    pub fn new(limit: Option<u32>, next_token: Option<String>) -> Self {
        Self {
            has_next: true,
            next_token,
            limit,
        }
    }

    /// Whether more items remain to fetch.
    pub fn has_next(&self) -> bool {
        self.has_next
    }

    /// The cursor for the next page, if any.
    pub fn next_token(&self) -> Option<&str> {
        self.next_token.as_deref()
    }

    /// The requested page-size hint, if any.
    pub fn limit(&self) -> Option<u32> {
        self.limit
    }

    /// Update from a response's `x-next-token` value. An empty or absent token
    /// ends pagination (`has_next` becomes `false`).
    pub fn update_from_token(&mut self, token: Option<String>) {
        self.next_token = token.filter(|t| !t.is_empty());
        self.has_next = self.next_token.is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_has_next_true() {
        let s = PaginationState::new(None, None);
        assert!(s.has_next());
        assert_eq!(s.next_token(), None);
        assert_eq!(s.limit(), None);
    }

    #[test]
    fn initial_token_and_limit_are_stored() {
        let s = PaginationState::new(Some(50), Some("cursor".to_string()));
        assert!(s.has_next());
        assert_eq!(s.next_token(), Some("cursor"));
        assert_eq!(s.limit(), Some(50));
    }

    #[test]
    fn nonempty_token_continues_pagination() {
        let mut s = PaginationState::new(None, None);
        s.update_from_token(Some("next".to_string()));
        assert!(s.has_next());
        assert_eq!(s.next_token(), Some("next"));
    }

    #[test]
    fn empty_or_missing_token_ends_pagination() {
        let mut s = PaginationState::new(None, Some("start".to_string()));
        s.update_from_token(Some(String::new()));
        assert!(!s.has_next());
        assert_eq!(s.next_token(), None);

        let mut s2 = PaginationState::new(None, Some("start".to_string()));
        s2.update_from_token(None);
        assert!(!s2.has_next());
        assert_eq!(s2.next_token(), None);
    }
}
