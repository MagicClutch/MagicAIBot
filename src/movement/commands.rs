use crate::{error::AppError, minecraft::world_state::PositionSnapshot};

pub fn parse_coordinates(arguments: &str) -> Result<PositionSnapshot, AppError> {
    let values: Vec<_> = arguments.split_whitespace().collect();
    if values.len() != 3 {
        return Err(AppError::InvalidCoordinates(
            "expected /goto <x> <y> <z>".into(),
        ));
    }
    let parse = |value: &str| {
        value
            .parse::<i32>()
            .map_err(|_| AppError::InvalidCoordinates(format!("invalid coordinate: {value}")))
    };
    let [x, y, z] = values.as_slice() else {
        unreachable!()
    };
    Ok(PositionSnapshot {
        x: f64::from(parse(x)?),
        y: f64::from(parse(y)?),
        z: f64::from(parse(z)?),
    })
}

pub fn parse_follow_name(arguments: &str) -> Result<String, AppError> {
    let name = arguments.trim();
    if name.is_empty() || name.split_whitespace().count() != 1 {
        return Err(AppError::InvalidConsoleSyntax("/follow <player>".into()));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_integer_coordinates() {
        assert_eq!(
            parse_coordinates("100 64 -20").unwrap(),
            PositionSnapshot {
                x: 100.,
                y: 64.,
                z: -20.
            }
        );
    }

    #[test]
    fn rejects_invalid_coordinates() {
        assert!(parse_coordinates("100 nope 20").is_err());
        assert!(parse_coordinates("100 64").is_err());
    }

    #[test]
    fn parses_one_follow_name() {
        assert_eq!(parse_follow_name("Steve").unwrap(), "Steve");
        assert!(parse_follow_name("Steve Alex").is_err());
    }
}
