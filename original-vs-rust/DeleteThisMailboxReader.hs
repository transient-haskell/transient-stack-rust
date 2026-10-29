{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Control.Monad
import Control.Applicative
import Control.Monad.IO.Class
import Control.Concurrent
import qualified System.Timeout as T

say :: String -> TransIO ()
say = liftIO . putStrLn

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 2000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' does not finish)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  run "M7 deleteThisMailboxReader leaves the other readers" $ do
    (do x <- getMailbox :: TransIO Int; say ("A " ++ show x))
      <|> (do x <- getMailbox :: TransIO Int; say ("B " ++ show x); deleteThisMailboxReader (0 :: Int))
      <|> (do putMailbox (1 :: Int); putMailbox (2 :: Int); deleteMailbox (0 :: Int); empty)
