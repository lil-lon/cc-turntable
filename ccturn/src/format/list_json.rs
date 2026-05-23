use crate::list::projects::ProjectListing;
use crate::list::sessions::SessionListing;

pub fn format_projects_json(listing: &ProjectListing) -> String {
    serde_json::to_string(listing).expect("ProjectListing serialization must not fail")
}

pub fn format_sessions_json(listing: &SessionListing) -> String {
    serde_json::to_string(listing).expect("SessionListing serialization must not fail")
}
